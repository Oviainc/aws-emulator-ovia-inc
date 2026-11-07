// SPDX-License-Identifier: CC0-1.0

//! The ONE precondition chain every Data API call is admitted by, before any statement
//! runs and before any transaction is opened, committed or rolled back.
//!
//! A Data API call names four things it does not own: the IDENTITY it signed as, the
//! CLUSTER it addresses, the SECRET it authenticates with and the DATABASE it runs
//! against. AWS resolves all four before the engine sees the statement, and refuses with
//! a different modelled class for each — which is what lets a client tell a configuration
//! problem from a credential problem from a SQL problem. This module is that chain, in
//! that order:
//!
//! 1. **the call's own shape** — a statement that is empty, and a `resourceArn` or
//!    `secretArn` that is not an ARN, are refused as validation faults BEFORE anything is
//!    resolved: there is nothing to resolve them against.
//! 2. **the identity** — [`super::authorization`], the identity-based IAM decision, so an
//!    unauthorized caller learns nothing about the cluster or the secret.
//! 3. **the cluster** — a statement runs on a cluster whose Data API HTTP endpoint is
//!    turned on. A `resourceArn` the RDS control plane holds no live cluster for (one
//!    never created, or one whose delete has been accepted) has no such endpoint either,
//!    so both are the same refusal: the Data API reports what it could not reach, not
//!    whether something exists in an account.
//! 4. **the secret** — the credentials the call authenticates with are fetched from
//!    Secrets Manager, so a secret that cannot be retrieved or does not hold credentials
//!    is refused before the statement runs.
//! 5. **the login** — those credentials have to be the cluster's: the database refuses a
//!    user it does not have, and the Data API passes that refusal through.
//! 6. **the database** — a statement runs against a database the cluster HAS.
//!
//! ## Nothing is assumed from silence
//!
//! Every fact above belongs to another service, and is read from the service that owns it:
//! the cluster from the Amazon RDS store (through its own `resolve_db_cluster`), the
//! secret from Secrets Manager (through `GetSecretValue`, because a secret's value is
//! sealed under the key the secret names and only its owner can open it), the identity
//! and its policies from the IAM and STS stores. An owner that CANNOT be asked — an
//! emulator assembled without it — leaves its question UNDECIDED, and an undecided
//! question admits the call rather than refusing it on the strength of a peer that never
//! answered. An owner that ANSWERS settles it.
//!
//! ## This is a read
//!
//! Admitting a call resolves facts and changes nothing: no cluster, database, secret or
//! transaction is created here, so a refused call leaves the store exactly as it was.

use std::collections::BTreeSet;

/// The error classes a call can be refused with before it runs, each the modelled class
/// AWS answers for that leg of the chain. The operation that was called turns one of
/// these into its own modelled error.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// The request itself is wrong, in a way the model's own constraints do not express:
    /// HTTP 400 `ValidationException`, the class the API's validation layer answers.
    Validation(String),
    /// The caller may not perform this action: HTTP 403 `AccessDeniedException`.
    AccessDenied(String),
    /// There is no Data API endpoint on the cluster the request addressed: HTTP 400
    /// `HttpEndpointNotEnabledException`.
    HttpEndpointNotEnabled(String),
    /// The secret could not be retrieved: HTTP 400 `SecretsErrorException`.
    SecretsError(String),
    /// The secret was retrieved and is not usable credentials: HTTP 400
    /// `InvalidSecretException`.
    InvalidSecret(String),
    /// The database refused the call — the login the secret carries, or the database the
    /// request named: HTTP 400 `DatabaseErrorException`.
    DatabaseError(String),
}

impl Refusal {
    /// What to tell the caller.
    pub fn message(&self) -> String {
        match self {
            Self::Validation(message)
            | Self::AccessDenied(message)
            | Self::HttpEndpointNotEnabled(message)
            | Self::SecretsError(message)
            | Self::InvalidSecret(message)
            | Self::DatabaseError(message) => message.clone(),
        }
    }
}

/// What one call asks for, as the chain reads it.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    /// The operation's own name ([`smithy_server_runtime::Operation::NAME`]), which the
    /// IAM action is formed from.
    pub operation: &'a str,
    pub resource_arn: &'a str,
    pub secret_arn: &'a str,
    /// The statement, for the two operations that carry one.
    pub sql: Option<&'a str>,
    /// The `database` member, for a call that selects one. A call inside a transaction
    /// passes `None`: the transaction already names the database it was opened for.
    pub database: Option<&'a str>,
}

/// The refusal the message for a cluster that has no Data API endpoint: "HttpEndpoint is
/// not enabled for resource " + the `resourceArn` the request carried + ".".
fn http_endpoint_not_enabled(resource_arn: &str) -> Refusal {
    Refusal::HttpEndpointNotEnabled(format!(
        "HttpEndpoint is not enabled for resource {resource_arn}."
    ))
}

/// The validation fault for a statement with nothing in it.
///
/// `sql` is required and `SqlStatement` carries no minimum length, so an empty statement
/// passes the model's own constraints and is refused here — ahead of resolving the
/// cluster, because there is no statement to run against one.
const EMPTY_SQL_MESSAGE: &str = "SQL is empty";

/// The user name the engine's access-denied diagnostic names when the secret carries
/// none: a login with no user name is a login as nobody.
const NO_USER_NAME: &str = "";

/// The address the data plane sees the Data API connect from. The Data API runs in this
/// process, so the connection it makes to the engine is a loopback one; the engine's own
/// access-denied diagnostic names the client address, which is this.
const DATA_API_CLIENT_ADDRESS: &str = "127.0.0.1";

/// The SQLSTATE of an invalid catalog name — a database that does not exist — from
/// PostgreSQL's "Error Codes" appendix (class 3D).
const SQLSTATE_INVALID_CATALOG_NAME: &str = "3D000";

/// How the Data API words a database that is not there: PostgreSQL's own sentence for the
/// condition, in the frame every error the database raises reaches the caller in — the
/// message, then the SQLSTATE a client classifies it by.
fn postgresql_catalog_error(database: &str) -> String {
    format!(
        "ERROR: database \"{database}\" does not exist; SQLState: \
         {SQLSTATE_INVALID_CATALOG_NAME}"
    )
}

/// Admit one call, or report the first leg of the chain that refuses it.
pub async fn admit<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    call: &Call<'_>,
) -> Result<(), Refusal> {
    let addressed = require_shape(call)?;
    super::authorization::authorize(engine, ctx, call.operation, call.resource_arn)
        .map_err(Refusal::AccessDenied)?;
    // The databases this service already holds for the cluster are read BEFORE anything
    // is awaited, so the chain takes this service's own lock once and never holds it
    // across an await point.
    let held = held_databases(engine, call.resource_arn);
    let cluster = describe_cluster(ctx, addressed.cluster_region, call.resource_arn).await;
    if let Some(cluster) = &cluster {
        if !cluster.serves_the_data_api {
            return Err(http_endpoint_not_enabled(call.resource_arn));
        }
    }
    let credentials =
        retrieve_credentials(ctx, addressed.secret_region, call.secret_arn).await?;
    if let (Some(cluster), Some(credentials)) = (&cluster, &credentials) {
        cluster.require_login(credentials)?;
    }
    if let (Some(cluster), Some(database)) = (&cluster, call.database) {
        cluster.require_database(database, &held)?;
    }
    Ok(())
}

/// What a `resourceArn` must name: a DB CLUSTER of Amazon RDS, the resource this API's
/// `resourceArn` is documented as "the Amazon Resource Name (ARN) of the Aurora
/// Serverless DB cluster". Read through the tree's one caller-supplied-ARN requirement
/// ([`::service_common::arn::Requirement`]), so an ARN of another vendor or of another
/// kind of RDS resource names no cluster here.
const RDS_CLUSTER: ::service_common::arn::Requirement = ::service_common::arn::Requirement {
    vendor: "rds",
    resource_type: Some("cluster"),
    label: "an Amazon RDS DB cluster",
};

/// What a `secretArn` must name, as the tree states that requirement once.
const SECRET: ::service_common::arn::Requirement =
    ::service_common::arn::SECRETS_MANAGER_SECRET;

/// What the two ARNs of an admitted call ADDRESS: the Region each resource is resolved
/// in, which is the Region its own ARN names.
#[derive(Debug, Clone, Copy)]
struct Addressed<'a> {
    cluster_region: &'a str,
    secret_region: &'a str,
}

/// Refuse a call whose own members cannot name anything, and answer where the two it
/// does name are resolved.
///
/// Both ARNs are read through [`::service_common::arn`], the tree's ONE
/// caller-supplied-ARN parse: a `resourceArn` has to be a well-formed Amazon RDS DB
/// CLUSTER ARN and a `secretArn` a Secrets Manager SECRET ARN, because this service
/// RESOLVES what each one names and a value of another shape or another vendor names
/// nothing. Each ARN's Region is read under that module's Region GRAMMAR rather than as
/// the field was submitted: this chain puts it in the authority of the endpoint it
/// resolves the resource through, so a field carrying `/`, `@`, `:`, `?` or `#` names no
/// Region at all.
fn require_shape<'call>(call: &Call<'call>) -> Result<Addressed<'call>, Refusal> {
    use ::service_common::arn;

    if call.sql.is_some_and(|sql| sql.trim().is_empty()) {
        return Err(Refusal::Validation(EMPTY_SQL_MESSAGE.to_string()));
    }
    let invalid_cluster = || {
        Refusal::Validation(format!("Invalid cluster arn: {}", call.resource_arn))
    };
    let cluster = arn::parse_of(call.resource_arn, RDS_CLUSTER.vendor).ok_or_else(invalid_cluster)?;
    if !RDS_CLUSTER.admits(call.resource_arn) {
        return Err(invalid_cluster());
    }
    let cluster_region = arn::region_label(cluster.region).ok_or_else(invalid_cluster)?;
    let secret_region = arn::region_of(call.secret_arn, SECRET.vendor)
        .filter(|_| SECRET.admits(call.secret_arn))
        .ok_or_else(|| {
            Refusal::Validation(format!("Invalid secret arn: {}", call.secret_arn))
        })?;
    // A cluster of ANOTHER account is a cluster this caller cannot address at all, and
    // saying so is decided from the ARN alone — nothing about another account's resources
    // is looked up, or could be.
    let account = ::engine::emulator_account_id();
    if !cluster.account.is_empty() && cluster.account != account {
        return Err(Refusal::Validation(format!(
            "The AWS account ID {account} doesn't own DB cluster {}.",
            call.resource_arn
        )));
    }
    Ok(Addressed {
        cluster_region,
        secret_region,
    })
}

/// What the Amazon RDS control plane answers about the cluster a request addressed.
#[derive(Debug, Clone)]
struct ClusterFacts {
    /// Whether there is a Data API HTTP endpoint on it at all: a live cluster with
    /// `EnableHttpEndpoint` turned on.
    serves_the_data_api: bool,
    /// `MasterUsername`: the user of the cluster, which is the login the Data API's
    /// secret has to carry. `None` for a cluster whose record names none, which decides
    /// nothing.
    master_username: Option<String>,
    /// `DatabaseName`: the database the cluster was created with, which it has from the
    /// moment it exists.
    database_name: Option<String>,
    /// `Engine`: which database engine the cluster runs, so the databases that engine
    /// ships with are known to be there too.
    engine: Option<String>,
}

impl ClusterFacts {
    /// A cluster the control plane ANSWERED about and holds nothing for: there is no Data
    /// API endpoint on it, and nothing is known about its user or its databases.
    fn absent() -> Self {
        Self {
            serves_the_data_api: false,
            master_username: None,
            database_name: None,
            engine: None,
        }
    }

    /// Refuse credentials the cluster has no user for.
    ///
    /// The Data API authenticates to the database with the secret's credentials, so a
    /// user name the cluster does not have is refused by the ENGINE, and the Data API
    /// passes that refusal through as `DatabaseErrorException`. The diagnostic is the
    /// engine's own: the user it tried, the address it came from, whether a password was
    /// supplied, and the engine's error and SQL state for an authentication failure.
    fn require_login(&self, credentials: &Credentials) -> Result<(), Refusal> {
        let Some(master) = &self.master_username else {
            return Ok(());
        };
        if credentials.username.as_deref() == Some(master.as_str()) {
            return Ok(());
        }
        Err(Refusal::DatabaseError(format!(
            "Access denied for user '{}'@'{DATA_API_CLIENT_ADDRESS}' (using password: {}); \
             Error code: 1045; SQLState: 28000",
            credentials.username.as_deref().unwrap_or(NO_USER_NAME),
            if credentials.has_password { "YES" } else { "NO" }
        )))
    }

    /// Refuse a `database` the cluster does not have.
    ///
    /// A statement runs against a database of the cluster: the one the cluster was
    /// created with, one the ENGINE it runs ships with ([`built_in_databases`]), or one
    /// this data plane already holds for it. Connecting to any other name is refused by
    /// the engine, in the engine's own words and with the SQLSTATE of an invalid catalog
    /// name, which the Data API passes through as `DatabaseErrorException`.
    ///
    /// A `database` member that is EMPTY names no database at all, so it is refused the
    /// same way: a call that means "the secret's own database" carries no member rather
    /// than an empty one.
    fn require_database(&self, database: &str, held: &BTreeSet<String>) -> Result<(), Refusal> {
        let has_it = !database.is_empty()
            && (held.contains(database)
                || built_in_databases(self.engine.as_deref()).contains(&database)
                || self
                    .database_name
                    .as_deref()
                    .is_some_and(|created_with| created_with == database));
        if has_it {
            return Ok(());
        }
        Err(Refusal::DatabaseError(postgresql_catalog_error(database)))
    }
}

/// Ask Amazon RDS about the cluster `resource_arn` names, in the Region that ARN names.
///
/// `DescribeDBClusters` takes "the user-supplied DB cluster identifier or the Amazon
/// Resource Name (ARN) of the DB cluster" and answers `DBClusterNotFoundFault` for one
/// that "doesn't refer to an existing DB cluster", which is exactly this question.
///
/// `None` when the question is UNDECIDED: the request reached no Amazon RDS at all — an
/// emulator assembled without that control plane knows of no cluster, so a call is
/// admitted rather than refused on the strength of a peer that was never asked. Any other
/// failure says nothing about the cluster either, and leaves it undecided too.
async fn describe_cluster(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_arn: &str,
) -> Option<ClusterFacts> {
    let answer = rds(ctx, region)
        .describe_db_clusters()
        .db_cluster_identifier(resource_arn)
        .send()
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            use ::aws_sdk_rds::error::ProvideErrorMetadata;
            if error
                .as_service_error()
                .is_some_and(::aws_sdk_rds::operation::describe_db_clusters::DescribeDBClustersError::is_db_cluster_not_found_fault)
            {
                // RDS was asked and holds no such cluster, which settles it: there is no
                // Data API endpoint on a cluster that is not there.
                return Some(ClusterFacts::absent());
            }
            if !error
                .code()
                .is_some_and(|code| TRANSPORT_REFUSALS.contains(&code))
            {
                // Visible at default verbosity: a statement admitted because the owner
                // could not be asked reads exactly like one admitted because the cluster
                // really serves the Data API, so the degraded path says so rather than
                // passing quietly.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the cluster a Data API call names could not be resolved"
                );
            }
            return None;
        }
    };
    // An answer carrying no cluster at all is an answer that the Region holds none of
    // that name.
    let Some(record) = answer.db_clusters().first() else {
        return Some(ClusterFacts::absent());
    };
    Some(ClusterFacts {
        serves_the_data_api: record.http_endpoint_enabled() == Some(true)
            && !being_deleted(record),
        master_username: record.master_username().map(str::to_string),
        database_name: record.database_name().map(str::to_string),
        engine: record.engine().map(str::to_string),
    })
}

/// The databases the ENGINE a cluster runs ships with, which it therefore has from the
/// moment it exists, beside the one its create named.
///
/// PostgreSQL creates `postgres` ("a default database meant for use by users, utilities
/// and third party applications") and the two templates `template0` / `template1` every
/// new database is cloned from, and RDS adds its own `rdsadmin`; MySQL creates `mysql`,
/// `information_schema`, `performance_schema` and `sys`. A statement naming one of them is
/// a statement against a database that is really there.
fn built_in_databases(engine: Option<&str>) -> &'static [&'static str] {
    const POSTGRESQL: &[&str] = &["postgres", "template0", "template1", "rdsadmin"];
    const MYSQL: &[&str] = &["mysql", "information_schema", "performance_schema", "sys"];
    let Some(engine) = engine else {
        return &[];
    };
    let engine = engine.to_ascii_lowercase();
    if engine.contains("postgres") {
        POSTGRESQL
    } else if engine.contains("mysql") || engine.contains("mariadb") {
        MYSQL
    } else {
        &[]
    }
}

/// Whether the cluster's delete has already been accepted — it reports the transient
/// `deleting` status. Such a cluster serves nothing: a delete "can't be canceled or
/// reverted after it begins".
fn being_deleted(record: &::aws_sdk_rds::types::DbCluster) -> bool {
    record.status() == Some(DB_CLUSTER_STATUS_DELETING)
}

/// The transient status a cluster reports between the delete being accepted and the
/// record going ("deleting", `DBCluster$Status`).
const DB_CLUSTER_STATUS_DELETING: &str = "deleting";

/// The Amazon RDS client a cluster is resolved through.
///
/// Configured exactly as the Secrets Manager client below is, and for the same reasons:
/// the callback's own HTTP client, so the endpoint URL is only the address that transport
/// routes on IN PROCESS, and the callback's credentials, which sign this in-process hop.
fn rds(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_rds::Client {
    let config = ::aws_sdk_rds::Config::builder()
        .behavior_version(::aws_sdk_rds::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_rds::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://rds.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_rds::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_rds::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_rds::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_rds::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_rds::Client::from_conf(config)
}

/// The databases this service already holds for `resource_arn` — the ones a statement
/// created by running against them.
///
/// A READ: the cluster is looked up rather than created, so admitting a call cannot bring
/// a cluster or a database into being, and a refused call leaves the store untouched.
fn held_databases<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_arn: &str,
) -> BTreeSet<String> {
    let Some(store) = super::service_state(engine) else {
        return BTreeSet::new();
    };
    let state = store.lock();
    state
        .clusters
        .get(resource_arn)
        .map(|cluster| cluster.databases.keys().cloned().collect())
        .unwrap_or_default()
}

/// The credentials a secret holds, as the Data API reads them.
#[derive(Debug, Clone)]
struct Credentials {
    /// The `username` entry of the secret's JSON document.
    username: Option<String>,
    /// Whether it carries a non-empty `password`, which is what the engine's diagnostic
    /// reports as "using password".
    has_password: bool,
}

/// The error codes that are the in-process TRANSPORT's own refusal rather than an answer
/// from Secrets Manager: this emulator holds no Secrets Manager at the endpoint that was
/// called, or more than one of its services claims it. Either way the request reached no
/// Secrets Manager, so it says nothing about the secret.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// What the Data API tells a caller whose secret Secrets Manager holds nothing for — one
/// of the conditions the modelled `SecretsErrorException` lists ("The secret provided
/// wasn't found"), naming the secret it looked for, which is the `secretArn` the request
/// carried.
fn secret_not_found_message(secret_arn: &str) -> String {
    format!("The secret {secret_arn} wasn't found.")
}

/// Another of that error's listed conditions: "The secret couldn't be decrypted."
const SECRET_NOT_DECRYPTED_MESSAGE: &str = "The secret couldn't be decrypted.";

/// What the Data API tells a caller whose secret is not a JSON document.
const SECRET_NOT_JSON_MESSAGE: &str = "The secret retrieved isn't valid JSON.";

/// What it tells one whose secret IS JSON but carries no credentials to log in with.
const SECRET_WITHOUT_CREDENTIALS_MESSAGE: &str =
    "The secret retrieved doesn't contain the username and password of a database user.";

/// The error Secrets Manager answers when it could not open a secret's value.
const SECRET_DECRYPTION_FAILURE_CODE: &str = "DecryptionFailure";

/// Fetch the credentials `secret_arn` names from Secrets Manager.
///
/// `Ok(None)` when the question is UNDECIDED: the request reached no Secrets Manager at
/// all, so nothing is known about the secret and the call is admitted (see the module
/// docs). Every answered failure refuses the call.
async fn retrieve_credentials(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_arn: &str,
) -> Result<Option<Credentials>, Refusal> {
    let answer = secrets_manager(ctx, region)
        .get_secret_value()
        .secret_id(secret_arn)
        .send()
        .await;
    let retrieved = match answer {
        Ok(retrieved) => retrieved,
        Err(error) => {
            use ::aws_sdk_secretsmanager::error::ProvideErrorMetadata;
            let code = error.code().map(str::to_string);
            if code
                .as_deref()
                .is_some_and(|code| TRANSPORT_REFUSALS.contains(&code))
            {
                ::tracing::debug!(
                    "no Secrets Manager answered, so the secret a Data API call names is \
                     unresolved"
                );
                return Ok(None);
            }
            // Secrets Manager ANSWERED, so the call is refused. Which of the modelled
            // conditions it is: a value the owner could not open is the undecryptable
            // one, and every other answered failure — an id it holds no secret for, a
            // read it refused — is a secret this call could not be given.
            let message = if code.as_deref() == Some(SECRET_DECRYPTION_FAILURE_CODE) {
                SECRET_NOT_DECRYPTED_MESSAGE.to_string()
            } else {
                secret_not_found_message(secret_arn)
            };
            return Err(Refusal::SecretsError(message));
        }
    };
    // A secret the Data API authenticates with holds its credentials as a JSON document:
    // "Enter the database user name and password for the credentials in the secret"
    // (`secretArn`, this service's Smithy model). A value in any other form is not one.
    let Some(document) = retrieved.secret_string() else {
        return Err(Refusal::InvalidSecret(SECRET_NOT_JSON_MESSAGE.to_string()));
    };
    let Ok(::serde_json::Value::Object(entries)) =
        ::serde_json::from_str::<::serde_json::Value>(document)
    else {
        return Err(Refusal::InvalidSecret(SECRET_NOT_JSON_MESSAGE.to_string()));
    };
    let entry = |name: &str| {
        entries
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string)
    };
    let username = entry("username");
    let password = entry("password");
    if username.is_none() && password.is_none() {
        return Err(Refusal::InvalidSecret(
            SECRET_WITHOUT_CREDENTIALS_MESSAGE.to_string(),
        ));
    }
    Ok(Some(Credentials {
        username,
        has_password: password.is_some_and(|password| !password.is_empty()),
    }))
}

/// The Secrets Manager client a secret is retrieved through.
///
/// `region` is the Region the secret's own ARN names, because Secrets Manager resolves a
/// secret only in the Region that holds it. The client is given the callback's own HTTP
/// client, so the endpoint URL is only the address that transport routes on IN PROCESS:
/// no socket is opened, no name is resolved and nothing leaves this process. The
/// credentials are the callback's, which sign this in-process hop and never appear in a
/// response. The transport is in-process, so there is nothing to retry, time out or watch
/// for a stalled stream — and each of those would need a sleep implementation this client
/// is not given.
fn secrets_manager(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_secretsmanager::Client {
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call with everything in place, which each row below breaks in one way.
    fn working_call() -> Call<'static> {
        Call {
            operation: "ExecuteStatement",
            resource_arn: "arn:aws:rds:us-east-1:000000000000:cluster:demo",
            secret_arn: "arn:aws:secretsmanager:us-east-1:000000000000:secret:demo-AbCdEf",
            sql: Some("SELECT 1"),
            database: None,
        }
    }

    /// RULE: a call whose own members name nothing is refused as a validation fault
    /// BEFORE anything is resolved, and the statement's own emptiness is decided first.
    ///
    /// The rows differ along WHICH member names nothing and HOW: a statement with nothing
    /// in it, a value that is no ARN at all, an ARN of another vendor, an ARN of another
    /// kind of that vendor's resources, an ARN whose Region field is no Region label (the
    /// field this chain puts in an endpoint's authority), and an ARN of another account.
    #[test]
    fn the_shape_of_a_call_is_decided_before_anything_is_resolved() {
        let account = ::engine::emulator_account_id();
        // (case, the call, the message the fault carries — `None` for a call that passes)
        let rows: Vec<(&str, Call<'_>, Option<String>)> = vec![
            ("a working call", working_call(), None),
            (
                "an empty statement",
                Call {
                    sql: Some(""),
                    ..working_call()
                },
                Some(EMPTY_SQL_MESSAGE.to_string()),
            ),
            (
                "a statement of blanks",
                Call {
                    sql: Some("   "),
                    ..working_call()
                },
                Some(EMPTY_SQL_MESSAGE.to_string()),
            ),
            (
                "a call that carries no statement at all",
                Call {
                    sql: None,
                    ..working_call()
                },
                None,
            ),
            (
                "a cluster that is no ARN",
                Call {
                    resource_arn: "demo",
                    ..working_call()
                },
                Some("Invalid cluster arn: demo".to_string()),
            ),
            (
                "a cluster ARN of another vendor",
                Call {
                    resource_arn: "arn:aws:redshift:us-east-1:000000000000:cluster:demo",
                    ..working_call()
                },
                Some(
                    "Invalid cluster arn: arn:aws:redshift:us-east-1:000000000000:cluster:demo"
                        .to_string(),
                ),
            ),
            (
                "an ARN of another kind of RDS resource",
                Call {
                    resource_arn: "arn:aws:rds:us-east-1:000000000000:db:demo",
                    ..working_call()
                },
                Some(
                    "Invalid cluster arn: arn:aws:rds:us-east-1:000000000000:db:demo".to_string(),
                ),
            ),
            (
                // The Region ends up in the authority of the endpoint this chain resolves
                // the cluster through, so a field that is no Region label names none.
                "a cluster ARN whose Region field is not a Region label",
                Call {
                    resource_arn: "arn:aws:rds:us-east-1/evil:000000000000:cluster:demo",
                    ..working_call()
                },
                Some(
                    "Invalid cluster arn: arn:aws:rds:us-east-1/evil:000000000000:cluster:demo"
                        .to_string(),
                ),
            ),
            (
                "a secret that is no ARN",
                Call {
                    secret_arn: "demo-secret",
                    ..working_call()
                },
                Some("Invalid secret arn: demo-secret".to_string()),
            ),
            (
                "a secret ARN of another vendor",
                Call {
                    secret_arn: "arn:aws:ssm:us-east-1:000000000000:parameter/demo",
                    ..working_call()
                },
                Some(
                    "Invalid secret arn: arn:aws:ssm:us-east-1:000000000000:parameter/demo"
                        .to_string(),
                ),
            ),
            (
                "a cluster of another account",
                Call {
                    resource_arn: "arn:aws:rds:us-east-1:111122223333:cluster:demo",
                    ..working_call()
                },
                Some(format!(
                    "The AWS account ID {account} doesn't own DB cluster \
                     arn:aws:rds:us-east-1:111122223333:cluster:demo."
                )),
            ),
            (
                // The empty statement is decided first, so it is what a call with both
                // faults reports.
                "an empty statement AND a cluster that is no ARN",
                Call {
                    sql: Some(""),
                    resource_arn: "demo",
                    ..working_call()
                },
                Some(EMPTY_SQL_MESSAGE.to_string()),
            ),
        ];
        for (case, call, expected) in rows {
            let refused = require_shape(&call).err().map(|refusal| refusal.message());
            assert_eq!(
                refused, expected,
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );
        }

        // A call that passes answers WHERE each resource is resolved: the Region its own
        // ARN names.
        let addressed = require_shape(&working_call()).expect("the working call is admitted");
        assert_eq!(
            (addressed.cluster_region, addressed.secret_region),
            ("us-east-1", "us-east-1"),
            "case a working call: expected each Region read from its own ARN, got \
             {addressed:?}"
        );
    }

    /// RULE: the login the secret carries has to be a user of the cluster, and a login
    /// the cluster has no user for is refused with the engine's own access-denied
    /// diagnostic.
    #[test]
    fn a_login_the_cluster_has_no_user_for_is_refused_with_the_engines_diagnostic() {
        let cluster = ClusterFacts {
            serves_the_data_api: true,
            master_username: Some("admin".to_string()),
            database_name: Some("demo".to_string()),
            engine: Some("aurora-postgresql".to_string()),
        };
        // (case, the secret's user, whether it carries a password, the refusal expected)
        let rows: &[(&str, Option<&str>, bool, Option<&str>)] = &[
            ("the cluster's own user", Some("admin"), true, None),
            (
                "the cluster's user without a password",
                Some("admin"),
                false,
                None,
            ),
            (
                "another user",
                Some("nobody"),
                true,
                Some(
                    "Access denied for user 'nobody'@'127.0.0.1' (using password: YES); \
                     Error code: 1045; SQLState: 28000",
                ),
            ),
            (
                "another user with no password",
                Some("nobody"),
                false,
                Some(
                    "Access denied for user 'nobody'@'127.0.0.1' (using password: NO); \
                     Error code: 1045; SQLState: 28000",
                ),
            ),
            (
                "the cluster's user in another case",
                Some("ADMIN"),
                true,
                Some(
                    "Access denied for user 'ADMIN'@'127.0.0.1' (using password: YES); \
                     Error code: 1045; SQLState: 28000",
                ),
            ),
            (
                "a secret naming no user",
                None,
                true,
                Some(
                    "Access denied for user ''@'127.0.0.1' (using password: YES); \
                     Error code: 1045; SQLState: 28000",
                ),
            ),
        ];
        for (case, username, has_password, expected) in rows {
            let refused = cluster
                .require_login(&Credentials {
                    username: username.map(str::to_string),
                    has_password: *has_password,
                })
                .err()
                .map(|refusal| refusal.message());
            assert_eq!(
                refused.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );
        }

        // A cluster whose record names no master user decides nothing, so every login is
        // admitted rather than refused on a fact the owner does not hold.
        let unknown_user = ClusterFacts {
            master_username: None,
            ..cluster
        };
        assert!(
            unknown_user
                .require_login(&Credentials {
                    username: Some("nobody".to_string()),
                    has_password: true,
                })
                .is_ok(),
            "case a cluster naming no master user: expected the login admitted"
        );
    }

    /// RULE: a statement runs against a database the cluster HAS — the one it was created
    /// with, or one this data plane already holds for it — and any other name is refused
    /// in the engine's own words.
    #[test]
    fn a_database_the_cluster_does_not_have_is_refused() {
        let cluster = ClusterFacts {
            serves_the_data_api: true,
            master_username: Some("admin".to_string()),
            database_name: Some("created_with".to_string()),
            engine: Some("aurora-postgresql".to_string()),
        };
        let held: BTreeSet<String> = ["already_held"].into_iter().map(str::to_string).collect();
        // (case, the database the call names, the refusal expected)
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("the database the cluster was created with", "created_with", None),
            ("a database this data plane already holds", "already_held", None),
            // The names PostgreSQL itself creates, which a cluster running it has.
            ("the engine's default database", "postgres", None),
            ("a template the engine creates", "template1", None),
            (
                "a member that is empty, which names no database",
                "",
                Some("ERROR: database \"\" does not exist; SQLState: 3D000"),
            ),
            (
                "a database nothing holds",
                "absent",
                Some("ERROR: database \"absent\" does not exist; SQLState: 3D000"),
            ),
            (
                "the created database in another case",
                "CREATED_WITH",
                Some("ERROR: database \"CREATED_WITH\" does not exist; SQLState: 3D000"),
            ),
            (
                // A name another engine creates is not a name THIS engine does.
                "another engine's own database",
                "information_schema",
                Some(
                    "ERROR: database \"information_schema\" does not exist; SQLState: 3D000",
                ),
            ),
        ];
        for (case, database, expected) in rows {
            let refused = cluster
                .require_database(database, &held)
                .err()
                .map(|refusal| refusal.message());
            assert_eq!(
                refused.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );
        }

        // The databases an engine ships with are the ones that engine documents, so a
        // cluster running MySQL has those instead.
        let mysql = ClusterFacts {
            engine: Some("aurora-mysql".to_string()),
            ..cluster
        };
        for (case, database, served) in [
            ("MySQL's own database", "mysql", true),
            ("MySQL's information schema", "information_schema", true),
            ("PostgreSQL's default database", "postgres", false),
        ] {
            let refused = mysql.require_database(database, &held).is_err();
            assert_eq!(
                refused, !served,
                "case {case:?}: expected served={served}, got refused={refused}"
            );
        }
    }

    /// RULE: a cluster the control plane holds nothing for, and one whose delete has been
    /// accepted, have no Data API endpoint — the same refusal, naming the ARN the request
    /// carried.
    #[test]
    fn the_endpoint_refusal_names_the_arn_the_request_carried() {
        for arn in [
            "arn:aws:rds:us-east-1:000000000000:cluster:demo",
            "arn:aws:rds:eu-west-1:000000000000:cluster:another-cluster",
        ] {
            let message = http_endpoint_not_enabled(arn).message();
            assert_eq!(
                message,
                format!("HttpEndpoint is not enabled for resource {arn}."),
                "case {arn:?}: expected AWS's wording, got {message:?}"
            );
        }
    }

    /// RULE: the Region a resource is resolved in is the one its own ARN names, read
    /// through the shared Region grammar.
    #[test]
    fn the_region_comes_from_the_arn() {
        /// One row: the case, the call, and the Regions its two ARNs name (`None` for a
        /// call whose own ARNs name no Region this chain can resolve in).
        type Row<'a> = (&'a str, Call<'a>, Option<(&'a str, &'a str)>);

        let rows: Vec<Row<'_>> = vec![
            ("the working call", working_call(), Some(("us-east-1", "us-east-1"))),
            (
                "resources in two Regions",
                Call {
                    resource_arn: "arn:aws:rds:eu-west-1:000000000000:cluster:demo",
                    ..working_call()
                },
                Some(("eu-west-1", "us-east-1")),
            ),
            (
                // A cluster ARN always names a Region, so a field that is empty names no
                // cluster this chain can resolve.
                "a cluster ARN with no Region at all",
                Call {
                    resource_arn: "arn:aws:rds::000000000000:cluster:demo",
                    ..working_call()
                },
                None,
            ),
        ];
        for (case, call, expected) in rows {
            let read = require_shape(&call)
                .ok()
                .map(|addressed| (addressed.cluster_region, addressed.secret_region));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
