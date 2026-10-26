// SPDX-License-Identifier: CC0-1.0

//! What an import file task INGESTS, and the read path that reports it, end to end over
//! HTTP.
//!
//! The rule under test is the one the operation states for any import file, not for one
//! recorded document: an import file task reads the object its `S3Bucket`/`s3key` name and
//! ingests the resources that document declares, so the TERMINAL STATUS it reaches reports
//! whether it brought the portfolio anything (`ImportSuccess` when it brought in at least
//! one of the records its file declared, `ImportFailed` when it brought in none or could
//! not read the file at all), `numberOfRecordsSuccess`/`numberOfRecordsFailed` count the
//! two halves of what the file declared,
//! and each ingested resource is on the service's read path from then on — a server in
//! `ListServers` (scoped by the import's own `groupIdFilter`), resolvable by its id through
//! `GetServerDetails`, carrying the baseline strategy `GetServerStrategies` reports; a
//! process or a database as an application component associated with the server its entry
//! names.
//!
//! The rows therefore span what the rule generalises over: how many resources the file
//! declares (none, one, several), which kinds it declares (servers, a process beside a
//! server, a database), whether an entry is one the service can import at all, how the
//! document spells its member names, and whether the import came in with groups. Nothing
//! below is asserted against one recorded file.
//!
//! The emulator mounted here holds the S3 service beside Strategy Recommendations on ONE
//! engine, exactly as the assembled emulator does: each case puts its import file through
//! S3's own front door and the import reads it back by calling S3 in process, so the whole
//! path a client takes is exercised.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::COMPLETION_SPAN;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, get_import, start_import, Answer};

/// The terminal `ImportFileTaskStatus` values an import reaches, spelled out here rather
/// than imported from the crate so a wrong status in `src/state.rs` fails here. From
/// `smithy_query(query="shape",shape="ImportFileTaskStatus")`.
const IMPORT_SUCCESS: &str = "ImportSuccess";
const IMPORT_FAILED: &str = "ImportFailed";

/// `Strategy`'s value for taking a workload to AWS as it is, from
/// `smithy_query(query="shape",shape="Strategy")` — the baseline a freshly ingested server
/// carries.
const STRATEGY_REHOST: &str = "Rehost";

/// Every value `StrategyRecommendation` declares, from
/// `smithy_query(query="shape",shape="StrategyRecommendation")`: the status a reported
/// strategy carries has to be one of them.
const STRATEGY_RECOMMENDATION: &[&str] = &["recommended", "viableOption", "notRecommended", "potential"];

/// Every value `OSType` declares, from `smithy_query(query="shape",shape="OSType")`.
const OS_TYPES: &[&str] = &["LINUX", "WINDOWS"];

/// Every value `ResourceSubType` declares, from
/// `smithy_query(query="shape",shape="ResourceSubType")`.
const RESOURCE_SUB_TYPES: &[&str] = &["Database", "Process", "DatabaseProcess"];

/// A SigV4 credential scope naming S3 in the same Region the Strategy Recommendations
/// requests of `crate::import_and_assess` are signed for, so the object this suite writes
/// is the object the import reads.
const SIGNED_FOR_S3_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The bucket every case puts its import file in; its name carries the
/// `migrationhub-strategy-` prefix each bucket member's documentation requires.
const IMPORT_BUCKET: &str = "migrationhub-strategy-import-files";

/// One emulator holding the S3 service beside Strategy Recommendations over one shared
/// engine — what the assembled emulator does.
fn emulator_with_object_storage() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One path-style request to the emulator's S3 front door.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: Bytes) -> (u16, Vec<u8>) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "s3.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_S3_EU_WEST_1)
        .body(body)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().as_ref().to_vec())
}

/// Create the import bucket through S3's own front door, as the account's owner would
/// before naming it to an import.
async fn create_bucket(case: &str, router: &MultiServiceRouter) {
    let payload = "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                   <LocationConstraint>eu-west-1</LocationConstraint>\
                   </CreateBucketConfiguration>";
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{IMPORT_BUCKET}"),
        Bytes::from_static(payload.as_bytes()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating the import bucket must succeed, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Put `document` in the import bucket under `key`, as the caller writing the import file
/// would.
async fn put_import_file(case: &str, router: &MultiServiceRouter, key: &str, document: &str) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{IMPORT_BUCKET}/{key}"),
        Bytes::from(document.to_string()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: writing the import file must succeed, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );
}

async fn list_servers(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-servers", Some(request)).await
}

async fn get_server_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-details/{id}"), None).await
}

async fn get_server_strategies(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-server-strategies/{id}"), None).await
}

async fn list_components(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-applicationcomponents", Some(request)).await
}

async fn get_component(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-details/{id}"),
        None,
    )
    .await
}

/// The entries of a reported list, or an empty list when it reported none.
fn entries(answer: &Answer, pointer: &str) -> Vec<Value> {
    at(&answer.body, pointer)
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `name`s a `ListServers` answer reported, sorted so a case asserts the SET it holds
/// rather than one order.
fn listed_names(answer: &Answer) -> Vec<String> {
    let mut names: Vec<String> = entries(answer, "/serverInfos")
        .iter()
        .map(|server| at(server, "/name").as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// The id of the server `name` in a `ListServers` answer.
fn listed_id(answer: &Answer, name: &str) -> String {
    entries(answer, "/serverInfos")
        .iter()
        .find(|server| at(server, "/name").as_str() == Some(name))
        .map(|server| at(server, "/id").as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

/// What one server entry of a file declares, and what the read path has to report for it.
struct ExpectedServer {
    name: &'static str,
    os_type: Option<&'static str>,
    os_version: Option<&'static str>,
    cpu_architecture: Option<&'static str>,
}

/// One row: an import file, the request that imports it, and what the import has to have
/// brought in.
struct Case {
    what: &'static str,
    /// The import document the case writes to S3.
    document: Value,
    /// The import groups the start call carries, which scope the imported records.
    groups: Vec<Value>,
    /// The servers the read path must report, with the members the file declared.
    servers: Vec<ExpectedServer>,
    /// The application components the import must have created, by name.
    components: Vec<&'static str>,
    /// How many entries of the file the import could not bring in.
    failed: i64,
    /// The terminal status that outcome gives the task.
    status: &'static str,
}

/// One SERVER entry of an import document.
fn server_entry(name: &str, id: &str, os_type: &str, os_version: &str, cpu: &str) -> Value {
    json!({
        "ResourceType": "SERVER",
        "ResourceId": id,
        "ResourceName": name,
        "OSType": os_type,
        "OSVersion": os_version,
        "CPUArchitecture": cpu,
    })
}

/// The rows, differing along what the rule generalises over: how many resources the file
/// declares and of which kinds, whether an entry is importable at all, the case its member
/// names are spelled in, and whether the import came in with groups.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "one server",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [server_entry(
                    "app-01",
                    "server-1",
                    "Linux",
                    "Ubuntu 20.04",
                    "64bit",
                )],
            }),
            groups: Vec::new(),
            servers: vec![ExpectedServer {
                name: "app-01",
                os_type: Some("LINUX"),
                os_version: Some("Ubuntu 20.04"),
                cpu_architecture: Some("64bit"),
            }],
            components: Vec::new(),
            failed: 0,
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "three servers, one of them Windows, under one import group",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [
                    server_entry("web-01", "server-1", "Windows", "WindowsServer 2019", "64bit"),
                    server_entry("web-02", "server-2", "Linux", "RHEL 8", "64bit"),
                    server_entry("db-01", "server-3", "Linux", "Amazon Linux 2", "arm64"),
                ],
            }),
            groups: vec![json!({"name": "ExternalId", "value": "wave-1"})],
            servers: vec![
                ExpectedServer {
                    name: "web-01",
                    os_type: Some("WINDOWS"),
                    os_version: Some("WindowsServer 2019"),
                    cpu_architecture: Some("64bit"),
                },
                ExpectedServer {
                    name: "web-02",
                    os_type: Some("LINUX"),
                    os_version: Some("RHEL 8"),
                    cpu_architecture: Some("64bit"),
                },
                ExpectedServer {
                    name: "db-01",
                    os_type: Some("LINUX"),
                    os_version: Some("Amazon Linux 2"),
                    cpu_architecture: Some("arm64"),
                },
            ],
            components: Vec::new(),
            failed: 0,
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a server with a process and a database running on it",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [
                    server_entry("app-02", "server-1", "Linux", "Ubuntu 22.04", "64bit"),
                    {
                        "ResourceType": "PROCESS",
                        "ResourceId": "process-1",
                        "ResourceName": "web-tier",
                        "AssociatedServerId": "server-1",
                        "AppType": "Tomcat",
                    },
                    {
                        "ResourceType": "DATABASE",
                        "ResourceId": "database-1",
                        "ResourceName": "orders-db",
                        "AssociatedServerId": "server-1",
                        "AppType": "MySQL",
                    },
                ],
            }),
            groups: vec![
                json!({"name": "ExternalId", "value": "wave-2"}),
                json!({"name": "ExternalSourceType", "value": "MPA"}),
            ],
            servers: vec![ExpectedServer {
                name: "app-02",
                os_type: Some("LINUX"),
                os_version: Some("Ubuntu 22.04"),
                cpu_architecture: Some("64bit"),
            }],
            components: vec!["orders-db", "web-tier"],
            failed: 0,
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a process declared before the server it runs on",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [
                    {
                        "ResourceType": "PROCESS",
                        "ResourceId": "process-1",
                        "ResourceName": "queue-worker",
                        "AssociatedServerId": "server-1",
                        "AppType": "Java",
                    },
                    server_entry("app-04", "server-1", "Linux", "Ubuntu 24.04", "arm64"),
                ],
            }),
            groups: Vec::new(),
            servers: vec![ExpectedServer {
                name: "app-04",
                os_type: Some("LINUX"),
                os_version: Some("Ubuntu 24.04"),
                cpu_architecture: Some("arm64"),
            }],
            components: vec!["queue-worker"],
            failed: 0,
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a document whose member names are spelled in another case",
            document: json!({
                "importFormatVersion": 1,
                "resources": [{
                    "resourcetype": "server",
                    "resourceid": "server-1",
                    "resourcename": "legacy-01",
                    "ostype": "windows",
                    "osversion": "WindowsServer 2012",
                }],
            }),
            groups: Vec::new(),
            servers: vec![ExpectedServer {
                name: "legacy-01",
                os_type: Some("WINDOWS"),
                os_version: Some("WindowsServer 2012"),
                cpu_architecture: None,
            }],
            components: Vec::new(),
            failed: 0,
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a file holding entries the service cannot import beside one it can",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [
                    server_entry("app-03", "server-1", "Linux", "Debian 12", "64bit"),
                    {"ResourceType": "NETWORK_SWITCH", "ResourceName": "switch-1"},
                    {"ResourceName": "no-type-at-all"},
                    {"ResourceType": "SERVER"},
                ],
            }),
            groups: Vec::new(),
            servers: vec![ExpectedServer {
                name: "app-03",
                os_type: Some("LINUX"),
                os_version: Some("Debian 12"),
                cpu_architecture: Some("64bit"),
            }],
            components: Vec::new(),
            failed: 3,
            // The import brought the portfolio a record, so it succeeded; the three
            // entries it could not bring in are what `numberOfRecordsFailed` reports.
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a file declaring no resource at all",
            document: json!({"ImportFormatVersion": 1, "Resources": []}),
            groups: Vec::new(),
            servers: Vec::new(),
            components: Vec::new(),
            failed: 0,
            // The file asked nothing of the import, and the import did all of it.
            status: IMPORT_SUCCESS,
        },
        Case {
            what: "a file none of whose entries can be imported",
            document: json!({
                "ImportFormatVersion": 1,
                "Resources": [
                    {"ResourceType": "NETWORK_SWITCH", "ResourceName": "switch-1"},
                    {"ResourceName": "no-type-at-all"},
                ],
            }),
            groups: Vec::new(),
            servers: Vec::new(),
            components: Vec::new(),
            failed: 2,
            status: IMPORT_FAILED,
        },
    ]
}

/// The `StartImportFileTask` request for one row.
fn import_request(index: usize, key: &str, groups: &[Value]) -> Value {
    let mut request = json!({
        "name": format!("ingestion-{index}"),
        "S3Bucket": IMPORT_BUCKET,
        "s3key": key,
    });
    if !groups.is_empty() {
        request["groupId"] = Value::Array(groups.to_vec());
    }
    request
}

/// Every record the import file declared is ingested: the completed task counts what it
/// brought in and what it could not, and each ingested resource is on the read path — the
/// servers through `ListServers`/`GetServerDetails`/`GetServerStrategies`, the processes and
/// databases through `ListApplicationComponents`/`GetApplicationComponentDetails`.
#[tokio::test]
async fn an_import_ingests_the_resources_its_file_declares() {
    for (index, case) in cases().into_iter().enumerate() {
        let what = case.what;
        // A fresh emulator per row, so the inventory a row reads holds only what ITS
        // import brought in.
        let router = emulator_with_object_storage();
        create_bucket(what, &router).await;
        let key = format!("inventory/import-{index}.json");
        put_import_file(what, &router, &key, &case.document.to_string()).await;

        let started = start_import(&router, import_request(index, &key, &case.groups)).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the import must start, got {} with {:?}",
            started.status, started.body
        );
        let task_id = started.string("/id");

        // The inventory a caller reads is the inventory of a FINISHED import, so nothing
        // is reported until the clock owes the task its terminal status.
        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        let reported = get_import(&router, &task_id).await;
        let expected_success =
            i64::try_from(case.servers.len() + case.components.len()).unwrap_or_default();
        assert_eq!(
            at(&reported.body, "/status"),
            json!(case.status),
            "case {what:?}: the task must report the terminal status its ingestion \
             produced, got {:?}",
            reported.body
        );
        assert_eq!(
            at(&reported.body, "/numberOfRecordsSuccess"),
            json!(expected_success),
            "case {what:?}: the import brought in {expected_success} record(s), got {:?}",
            at(&reported.body, "/numberOfRecordsSuccess")
        );
        assert_eq!(
            at(&reported.body, "/numberOfRecordsFailed"),
            json!(case.failed),
            "case {what:?}: {} entries could not be imported, got {:?}",
            case.failed,
            at(&reported.body, "/numberOfRecordsFailed")
        );

        // The servers the file declared are the inventory the read path reports.
        let listed = list_servers(&router, json!({})).await;
        assert_eq!(listed.status, 200, "case {what:?}: the listing must succeed, got {:?}", listed.body);
        let mut expected_names: Vec<String> = case
            .servers
            .iter()
            .map(|server| server.name.to_string())
            .collect();
        expected_names.sort();
        assert_eq!(
            listed_names(&listed),
            expected_names,
            "case {what:?}: ListServers must report the servers the import ingested, got {:?}",
            listed.body
        );

        for expected in &case.servers {
            let name = expected.name;
            let id = listed_id(&listed, name);
            assert!(
                !id.is_empty() && id.len() <= 27,
                "case {what:?}: server {name:?} must be reported under a ServerId \
                 (1..=27 characters), got {id:?}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_digit() || c == 'a' || c == 'b'),
                "case {what:?}: server {name:?} must be reported under a ResourceId \
                 (^[0-9a-b]+), got {id:?}"
            );

            // The single-server read resolves the same record the listing reported.
            let detail = get_server_details(&router, &id).await;
            assert_eq!(
                detail.status, 200,
                "case {what:?}: GetServerDetails must resolve {id:?}, got {} with {:?}",
                detail.status, detail.body
            );
            for (pointer, declared) in [
                ("/serverDetail/name", Some(name)),
                ("/serverDetail/systemInfo/osInfo/type", expected.os_type),
                (
                    "/serverDetail/systemInfo/osInfo/version",
                    expected.os_version,
                ),
                (
                    "/serverDetail/systemInfo/cpuArchitecture",
                    expected.cpu_architecture,
                ),
            ] {
                let expected_value = declared.map_or(Value::Null, |text| json!(text));
                assert_eq!(
                    at(&detail.body, pointer),
                    expected_value,
                    "case {what:?}: {pointer} must report what the file declared for \
                     {name:?}, got {:?}",
                    at(&detail.body, pointer)
                );
            }
            if let Some(os_type) = expected.os_type {
                assert!(
                    OS_TYPES.contains(&os_type),
                    "case {what:?}: this row expects an OSType the model does not declare: \
                     {os_type:?}"
                );
            }

            // And it carries the baseline strategy an ingested server holds before any
            // assessment has analysed it.
            let strategies = get_server_strategies(&router, &id).await;
            assert_eq!(
                strategies.status, 200,
                "case {what:?}: GetServerStrategies must answer for {id:?}, got {} with {:?}",
                strategies.status, strategies.body
            );
            let listings = entries(&strategies, "/serverStrategies");
            assert!(
                !listings.is_empty(),
                "case {what:?}: an ingested server carries a strategy, got {:?}",
                strategies.body
            );
            let rehost = listings
                .iter()
                .find(|listing| {
                    at(listing, "/recommendation/strategy").as_str() == Some(STRATEGY_REHOST)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {what:?}: the baseline for {name:?} is a {STRATEGY_REHOST} \
                         recommendation, got {:?}",
                        strategies.body
                    )
                });
            let status = at(rehost, "/status").as_str().unwrap_or_default().to_string();
            assert!(
                STRATEGY_RECOMMENDATION.contains(&status.as_str()),
                "case {what:?}: the baseline strategy of {name:?} must carry a \
                 StrategyRecommendation status, got {status:?}"
            );
            assert!(
                at(rehost, "/recommendation/targetDestination")
                    .as_str()
                    .is_some_and(|destination| !destination.is_empty()),
                "case {what:?}: the baseline strategy of {name:?} names where it moves \
                 the server, got {rehost:?}"
            );
        }

        // The processes and databases the file declared are application components, each
        // associated with the server its entry named.
        let components = list_components(&router, json!({})).await;
        let mut component_names: Vec<String> = entries(&components, "/applicationComponentInfos")
            .iter()
            .map(|component| {
                at(component, "/name")
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        component_names.sort();
        let expected_components: Vec<String> = case
            .components
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        assert_eq!(
            component_names, expected_components,
            "case {what:?}: ListApplicationComponents must report the components the import \
             ingested, got {:?}",
            components.body
        );
        for component in entries(&components, "/applicationComponentInfos") {
            let id = at(&component, "/id").as_str().unwrap_or_default().to_string();
            let detail = get_component(&router, &id).await;
            assert_eq!(
                detail.status, 200,
                "case {what:?}: GetApplicationComponentDetails must resolve {id:?}, got {} \
                 with {:?}",
                detail.status, detail.body
            );
            let sub_type = detail.string("/applicationComponentDetail/resourceSubType");
            assert!(
                RESOURCE_SUB_TYPES.contains(&sub_type.as_str()),
                "case {what:?}: component {id:?} must carry a ResourceSubType the model \
                 declares, got {sub_type:?}"
            );
            let associated = entries(&detail, "/associatedServerIds");
            let server_ids: Vec<String> = case
                .servers
                .iter()
                .map(|server| listed_id(&listed, server.name))
                .collect();
            assert!(
                associated
                    .iter()
                    .all(|server| server_ids.contains(&server.as_str().unwrap_or_default().to_string())),
                "case {what:?}: component {id:?} must be associated with the server its entry \
                 named, got {associated:?} against {server_ids:?}"
            );
            assert_eq!(
                associated.len(),
                1,
                "case {what:?}: component {id:?} runs on the one server its entry named, got \
                 {associated:?}"
            );
        }
    }
}

/// `groupIdFilter` scopes the inventory to the servers that came in with the groups it
/// names: an import's own group selects exactly that import's servers, a group nothing was
/// imported under selects none, and an unfiltered call reports the whole account.
#[tokio::test]
async fn an_imported_server_is_discoverable_by_its_own_group() {
    let router = emulator_with_object_storage();
    create_bucket("group scoping", &router).await;

    // Two imports into one account, each under its own group, so a filter that ignored the
    // groups would report the other import's server as well.
    let imports: &[(&str, &str, &str)] = &[
        ("group-a", "wave-a", "server-a"),
        ("group-b", "wave-b", "server-b"),
    ];
    for (index, (key, group, server_name)) in imports.iter().enumerate() {
        let document = json!({
            "ImportFormatVersion": 1,
            "Resources": [server_entry(server_name, "server-1", "Linux", "RHEL 9", "64bit")],
        });
        put_import_file("group scoping", &router, key, &document.to_string()).await;
        let started = start_import(
            &router,
            import_request(
                index,
                key,
                &[json!({"name": "ExternalId", "value": group})],
            ),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {group:?}: the import must start, got {} with {:?}",
            started.status, started.body
        );
    }
    tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

    // Each import's own group selects its own server and nothing else.
    for (_, group, server_name) in imports {
        let filtered = list_servers(
            &router,
            json!({"groupIdFilter": [{"name": "ExternalId", "value": group}]}),
        )
        .await;
        assert_eq!(
            listed_names(&filtered),
            vec![(*server_name).to_string()],
            "case {group:?}: a server imported under a group must be discoverable by that \
             group's own filter, got {:?}",
            filtered.body
        );
    }

    // A group nothing was imported under selects nothing.
    let unused = list_servers(
        &router,
        json!({"groupIdFilter": [{"name": "ExternalSourceType", "value": "never-used"}]}),
    )
    .await;
    assert_eq!(
        listed_names(&unused),
        Vec::<String>::new(),
        "case \"a group nothing was imported under\": the filter must select no server, got {:?}",
        unused.body
    );

    // And an unfiltered call reports both imports' servers.
    let all = list_servers(&router, json!({})).await;
    assert_eq!(
        listed_names(&all),
        vec!["server-a".to_string(), "server-b".to_string()],
        "case \"no filter\": the whole account's inventory is reported, got {:?}",
        all.body
    );
}

/// An import whose file cannot be read brings in nothing and says so: the task reaches the
/// terminal status for an import that read none of what it was asked to read
/// (`ImportFailed`), its record counts are zero rather than a count nothing read, and no
/// server appears in the inventory.
#[tokio::test]
async fn an_import_file_that_cannot_be_read_fails_the_task() {
    let cases: &[(&str, Option<&str>)] = &[
        ("no object under the key at all", None),
        ("an object that is not an import document", Some("not json")),
        (
            "a document carrying no list of resources",
            Some("{\"ImportFormatVersion\":1}"),
        ),
    ];
    for (index, (what, body)) in cases.iter().enumerate() {
        let router = emulator_with_object_storage();
        create_bucket(what, &router).await;
        let key = format!("unreadable-{index}.json");
        if let Some(document) = body {
            put_import_file(what, &router, &key, document).await;
        }

        let started = start_import(&router, import_request(index, &key, &[])).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the import must still start, got {} with {:?}",
            started.status, started.body
        );
        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        let reported = get_import(&router, &started.string("/id")).await;
        assert_eq!(
            at(&reported.body, "/status"),
            json!(IMPORT_FAILED),
            "case {what:?}: an import that could not read its file must report \
             {IMPORT_FAILED}, got {:?}",
            reported.body
        );
        for member in ["/numberOfRecordsSuccess", "/numberOfRecordsFailed"] {
            assert_eq!(
                at(&reported.body, member),
                json!(0),
                "case {what:?}: nothing was read, so {member} must report zero, got {:?}",
                at(&reported.body, member)
            );
        }
        assert_eq!(
            listed_names(&list_servers(&router, json!({})).await),
            Vec::<String>::new(),
            "case {what:?}: an import that read nothing brings no server into the inventory"
        );
    }
}

/// The PROCESS entry one import file declares, naming the server it runs on with
/// `reference`.
fn process_entry(name: &str, id: &str, reference: &str) -> Value {
    json!({
        "ResourceType": "PROCESS",
        "ResourceId": id,
        "ResourceName": name,
        "AssociatedServerId": reference,
        "AppType": "Tomcat",
    })
}

/// The import document is a CUSTOMER's document rather than a modelled wire shape: the
/// servers an application component runs on are carried as a LIST
/// (`AssociatedServerIds`), because a component can run on several — which is what
/// `ApplicationComponentDetail$moreServerAssociationExists` reports — beside the singular
/// `AssociatedServerId` for the one-server case, and the application type under the names
/// the format carries it by. So an entry's attributes are read however the document spells
/// them, and a component is associated with EVERY server its entry names.
#[tokio::test]
async fn an_entrys_attributes_are_read_however_the_document_spells_them() {
    /// One component entry: the member carrying the servers it runs on and the value of
    /// that member, the member carrying its application type, and which of the file's
    /// servers it must come out associated with.
    struct Row {
        what: &'static str,
        server_member: &'static str,
        servers: Value,
        type_member: &'static str,
        app_type: &'static str,
        on: &'static [&'static str],
    }

    let what = "the spellings of an entry's attributes";
    let router = emulator_with_object_storage();
    create_bucket(what, &router).await;

    // Rows: the singular member naming one server by its resource id, the list member with
    // one entry, the list member with TWO servers, a list naming a server by its NAME, and
    // each member name spelled in another case — with both spellings of the application
    // type across them.
    let rows: &[Row] = &[
        Row {
            what: "the singular member, by resource id",
            server_member: "AssociatedServerId",
            servers: json!("server-1"),
            type_member: "AppType",
            app_type: "IIS",
            on: &["host-01"],
        },
        Row {
            what: "the list member with one entry",
            server_member: "AssociatedServerIds",
            servers: json!(["server-2"]),
            type_member: "ApplicationType",
            app_type: "Tomcat",
            on: &["host-02"],
        },
        Row {
            what: "the list member naming both servers",
            server_member: "AssociatedServerIds",
            servers: json!(["server-1", "server-2"]),
            type_member: "AppType",
            app_type: "JBoss",
            on: &["host-01", "host-02"],
        },
        Row {
            what: "the list member, by the server's name, spelled in another case",
            server_member: "associatedserverids",
            servers: json!(["host-02"]),
            type_member: "applicationtype",
            app_type: "Oracle",
            on: &["host-02"],
        },
        Row {
            what: "the singular member spelled in another case",
            server_member: "associatedserverid",
            servers: json!("host-01"),
            type_member: "apptype",
            app_type: "Spring",
            on: &["host-01"],
        },
    ];

    let mut resources = vec![
        server_entry("host-01", "server-1", "Linux", "Ubuntu 22.04", "64bit"),
        server_entry("host-02", "server-2", "Windows", "WindowsServer 2019", "64bit"),
    ];
    for (index, row) in rows.iter().enumerate() {
        resources.push(json!({
            "ResourceType": "PROCESS",
            "ResourceId": format!("process-{index}"),
            "ResourceName": format!("component-{index}"),
            row.server_member: row.servers,
            row.type_member: row.app_type,
        }));
    }

    let key = "spellings/import.json";
    let document = json!({"ImportFormatVersion": 1, "Resources": resources});
    put_import_file(what, &router, key, &document.to_string()).await;
    let started = start_import(&router, import_request(0, key, &[])).await;
    assert_eq!(
        started.status, 200,
        "case {what:?}: the import must start, got {} with {:?}",
        started.status, started.body
    );
    tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

    let reported = get_import(&router, &started.string("/id")).await;
    let expected_success = i64::try_from(rows.len() + 2).unwrap_or_default();
    assert_eq!(
        at(&reported.body, "/numberOfRecordsSuccess"),
        json!(expected_success),
        "case {what:?}: every entry must be brought in, got {:?}",
        reported.body
    );
    assert_eq!(
        at(&reported.body, "/numberOfRecordsFailed"),
        json!(0),
        "case {what:?}: no entry should have failed, got {:?}",
        reported.body
    );

    let listed = list_servers(&router, json!({})).await;
    let components = entries(
        &list_components(&router, json!({})).await,
        "/applicationComponentInfos",
    );
    assert_eq!(
        components.len(),
        rows.len(),
        "case {what:?}: every component entry must be reported, got {components:?}"
    );
    for (index, row) in rows.iter().enumerate() {
        let case = row.what;
        let name = format!("component-{index}");
        let component = components
            .iter()
            .find(|reported| at(reported, "/name").as_str() == Some(name.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {name:?} must be reported, got {components:?}")
            });
        let expected_ids: Vec<String> = row
            .on
            .iter()
            .map(|server| listed_id(&listed, server))
            .collect();

        // The detail reports ONE associated server — it is a single member — and flags
        // that there are more when the entry named more.
        let reported_server = at(component, "/associatedServerId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            expected_ids.contains(&reported_server),
            "case {case:?}: expected the component to run on one of {expected_ids:?}, got \
             {reported_server:?}"
        );
        assert_eq!(
            at(component, "/moreServerAssociationExists"),
            json!(expected_ids.len() > 1),
            "case {case:?}: expected moreServerAssociationExists {}, got {:?}",
            expected_ids.len() > 1,
            at(component, "/moreServerAssociationExists")
        );
        assert_eq!(
            at(component, "/appType"),
            json!(row.app_type),
            "case {case:?}: expected the declared appType {:?}, got {:?}",
            row.app_type,
            at(component, "/appType")
        );

        // And the single-component read reports EVERY server the entry named.
        let component_id = at(component, "/id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let detail = get_component(&router, &component_id).await;
        let mut associated: Vec<String> = entries(&detail, "/associatedServerIds")
            .iter()
            .map(|server| server.as_str().unwrap_or_default().to_string())
            .collect();
        associated.sort();
        let mut expected_sorted = expected_ids.clone();
        expected_sorted.sort();
        assert_eq!(
            associated, expected_sorted,
            "case {case:?}: expected the component to run on {expected_sorted:?}, got \
             {associated:?}"
        );
    }
}

/// The resource id or name an import file declares for a server is the account's own name
/// for that machine, so an application component declaring that it runs on that server is
/// associated with it WHEREVER the server was declared — including by an import that
/// already brought it into the account — while a component naming a server NOTHING
/// declared is a record the import could not bring in: there is no server to attach it to.
#[tokio::test]
async fn a_component_runs_on_the_server_an_earlier_import_declared() {
    /// One row: how the second import's process names the server the first import
    /// declared, and whether that names a server of the account at all.
    struct Row {
        what: &'static str,
        reference: &'static str,
        associated: bool,
    }

    // Rows: the two ways a server is declared — the resource id and the name — and a
    // reference no import declared under either.
    let rows = [
        Row {
            what: "the resource id the earlier import declared the server under",
            reference: "server-1",
            associated: true,
        },
        Row {
            what: "the name the earlier import declared the server under",
            reference: "host-01",
            associated: true,
        },
        Row {
            what: "a reference no import declared",
            reference: "host-absent",
            associated: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let what = row.what;
        // A fresh emulator per row: the only server the account holds is the one the first
        // import of this row declares.
        let router = emulator_with_object_storage();
        create_bucket(what, &router).await;

        // The first import declares the server and nothing else.
        let server_key = format!("across/servers-{index}.json");
        let servers = json!({
            "ImportFormatVersion": 1,
            "Resources": [server_entry("host-01", "server-1", "Linux", "Ubuntu 22.04", "64bit")],
        });
        put_import_file(what, &router, &server_key, &servers.to_string()).await;
        let started = start_import(&router, import_request(2 * index, &server_key, &[])).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the server import must start, got {} with {:?}",
            started.status, started.body
        );
        let server_task = started.string("/id");

        // Its records are the account's once the clock owes that import its terminal
        // status, which is when a client would start the next import.
        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        // The second declares only the process, naming the server of the first.
        let process_key = format!("across/processes-{index}.json");
        let processes = json!({
            "ImportFormatVersion": 1,
            "Resources": [process_entry("web-tier", "process-1", row.reference)],
        });
        put_import_file(what, &router, &process_key, &processes.to_string()).await;
        let started = start_import(&router, import_request(2 * index + 1, &process_key, &[])).await;
        assert_eq!(
            started.status, 200,
            "case {what:?}: the process import must start, got {} with {:?}",
            started.status, started.body
        );
        let process_task = started.string("/id");
        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        // The first import brought in its one server; the second brought in its component
        // only where the server it names is one the account holds.
        let reported = get_import(&router, &server_task).await;
        assert_eq!(
            at(&reported.body, "/status"),
            json!(IMPORT_SUCCESS),
            "case {what:?}: the server import must report {IMPORT_SUCCESS}, got {:?}",
            reported.body
        );
        assert_eq!(
            at(&reported.body, "/numberOfRecordsSuccess"),
            json!(1),
            "case {what:?}: the server import brought in its one record, got {:?}",
            at(&reported.body, "/numberOfRecordsSuccess")
        );

        let reported = get_import(&router, &process_task).await;
        let (expected_status, expected_success, expected_failed) = if row.associated {
            (IMPORT_SUCCESS, 1, 0)
        } else {
            (IMPORT_FAILED, 0, 1)
        };
        assert_eq!(
            at(&reported.body, "/status"),
            json!(expected_status),
            "case {what:?}: the process import must report {expected_status}, got {:?}",
            reported.body
        );
        assert_eq!(
            at(&reported.body, "/numberOfRecordsSuccess"),
            json!(expected_success),
            "case {what:?}: expected {expected_success} record(s) imported, got {:?}",
            at(&reported.body, "/numberOfRecordsSuccess")
        );
        assert_eq!(
            at(&reported.body, "/numberOfRecordsFailed"),
            json!(expected_failed),
            "case {what:?}: expected {expected_failed} record(s) failed, got {:?}",
            at(&reported.body, "/numberOfRecordsFailed")
        );

        let server_id = listed_id(&list_servers(&router, json!({})).await, "host-01");
        assert!(
            !server_id.is_empty(),
            "case {what:?}: the first import's server must be in the inventory"
        );

        let components = entries(
            &list_components(&router, json!({})).await,
            "/applicationComponentInfos",
        );
        if !row.associated {
            assert!(
                components.is_empty(),
                "case {what:?}: a record the import could not bring in must materialise no \
                 component, got {components:?}"
            );
            continue;
        }
        assert_eq!(
            components.len(),
            1,
            "case {what:?}: the process import brought in one component, got {components:?}"
        );
        let component = &components[0];
        assert_eq!(
            at(component, "/associatedServerId"),
            json!(server_id.clone()),
            "case {what:?}: expected associatedServerId {server_id:?}, got {:?}",
            at(component, "/associatedServerId")
        );

        // The single-component read reports the same association beside the detail.
        let component_id = at(component, "/id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let detail = get_component(&router, &component_id).await;
        let associated = entries(&detail, "/associatedServerIds");
        assert_eq!(
            associated,
            vec![json!(server_id)],
            "case {what:?}: expected the component to report the server it runs on, got \
             {associated:?}"
        );
    }
}

/// A file that declares BOTH the server and the component running on it is
/// self-contained: the component is associated with that file's own server, even where the
/// account already holds another server declared under the same resource id, so an
/// association is never taken from an earlier import over the document being imported.
#[tokio::test]
async fn a_file_that_declares_the_server_it_names_resolves_its_own() {
    let what = "a resource id two imports both declare";
    let router = emulator_with_object_storage();
    create_bucket(what, &router).await;

    // The first import declares `server-1` as one machine.
    let first_key = "both/first.json";
    let first = json!({
        "ImportFormatVersion": 1,
        "Resources": [server_entry("host-01", "server-1", "Linux", "Ubuntu 22.04", "64bit")],
    });
    put_import_file(what, &router, first_key, &first.to_string()).await;
    let started = start_import(&router, import_request(0, first_key, &[])).await;
    assert_eq!(
        started.status, 200,
        "case {what:?}: the first import must start, got {} with {:?}",
        started.status, started.body
    );
    tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

    // The second declares ANOTHER machine under the same resource id, and a process on it.
    let second_key = "both/second.json";
    let second = json!({
        "ImportFormatVersion": 1,
        "Resources": [
            server_entry("host-02", "server-1", "Linux", "RHEL 9", "arm64"),
            process_entry("web-tier", "process-1", "server-1"),
        ],
    });
    put_import_file(what, &router, second_key, &second.to_string()).await;
    let started = start_import(&router, import_request(1, second_key, &[])).await;
    assert_eq!(
        started.status, 200,
        "case {what:?}: the second import must start, got {} with {:?}",
        started.status, started.body
    );
    tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

    let listed = list_servers(&router, json!({})).await;
    let first_id = listed_id(&listed, "host-01");
    let second_id = listed_id(&listed, "host-02");
    assert!(
        !first_id.is_empty() && !second_id.is_empty() && first_id != second_id,
        "case {what:?}: each import's server is a record of its own, got {first_id:?} and \
         {second_id:?}"
    );

    let components = entries(
        &list_components(&router, json!({})).await,
        "/applicationComponentInfos",
    );
    assert_eq!(
        components.len(),
        1,
        "case {what:?}: the second import brought in one component, got {components:?}"
    );
    assert_eq!(
        at(&components[0], "/associatedServerId"),
        json!(second_id.clone()),
        "case {what:?}: expected the component to run on its own file's server \
         {second_id:?}, got {:?}",
        at(&components[0], "/associatedServerId")
    );
}
