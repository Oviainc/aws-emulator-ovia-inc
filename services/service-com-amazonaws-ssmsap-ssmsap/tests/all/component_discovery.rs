// SPDX-License-Identifier: CC0-1.0

//! The components of a registered application come from DISCOVERY, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: what a registration left behind is only believed when a
//! later `ListComponents` / `GetApplication` / `GetComponent` call reports it back.
//!
//! RULE under test: a component of a registered application exists only once the service's
//! discovery of that application has produced it. Registration is asynchronous and
//! discovery runs as part of it (`Application$DiscoveryStatus` is "the latest discovery
//! result for the application", `REGISTERING` while it runs), so while the registration is
//! still under way EVERY reader reports no component for it — the listing answers 200 with
//! an empty set rather than the inventory the request declared — and once the discovery
//! has completed every reader reports exactly the components it produced, for as long as
//! the application exists.
//!
//! The rows span what the rule generalises over: how many components the registration
//! describes (none at all, one, two, and the five the model's `ComponentInfoList` admits),
//! both `ApplicationType`s, the readers (`ListComponents` narrowed and over the whole
//! Region, `GetApplication`, `GetComponent`), how often a caller polls during the
//! discovery window, and the end of the application's life.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::COMPONENT_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_DECLARED_COMPONENTS;

use crate::application_lifecycle::{settle, string_list, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::VALIDATION_EXCEPTION;

/// The regional endpoint every row signs for.
const REGION: &str = "us-east-1";

/// The Amazon EC2 instance `valid_registration` registers an application against, so a
/// declared component runs on an instance the registration names.
const INSTANCE: &str = "i-0123456789abcdef0";

/// One registration, and the components its discovery must produce for it.
#[derive(Debug)]
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `ComponentsInfo` entries, as (`ComponentType`, `Sid`).
    declared: &'static [(&'static str, &'static str)],
}

/// The rows: 0, 1, 2 and the model's maximum of 5 declared components, over both
/// application types, with `ComponentType`s taken from the model's own catalogue.
const CASES: &[Case] = &[
    Case {
        name: "a HANA registration declaring nothing",
        application_id: "discovery-none",
        application_type: "HANA",
        declared: &[],
    },
    Case {
        name: "an ABAP registration declaring one component",
        application_id: "discovery-one",
        application_type: "SAP_ABAP",
        declared: &[("ASCS", "PRD")],
    },
    Case {
        name: "a HANA registration declaring two components",
        application_id: "discovery-two",
        application_type: "HANA",
        declared: &[("HANA_NODE", "HDB"), ("WEBDISP", "WD1")],
    },
    Case {
        name: "an ABAP registration declaring the five the model admits",
        application_id: "discovery-five",
        application_type: "SAP_ABAP",
        declared: &[
            ("ABAP", "AB1"),
            ("ASCS", "AS1"),
            ("DIALOG", "DI1"),
            ("ERS", "ER1"),
            ("WD", "WD1"),
        ],
    },
];

impl Case {
    /// The `RegisterApplication` body this row sends.
    fn request(&self) -> Value {
        let mut body = valid_registration(self.application_id, self.application_type);
        let object = body.as_object_mut().expect("the body is a JSON object");
        if !self.declared.is_empty() {
            object.insert(
                "ComponentsInfo".to_string(),
                Value::Array(
                    self.declared
                        .iter()
                        .map(|(component_type, sid)| {
                            json!({
                                "ComponentType": component_type,
                                "Sid": sid,
                                "Ec2InstanceId": INSTANCE,
                            })
                        })
                        .collect(),
                ),
            );
        }
        body
    }

    /// The `ComponentId`s discovery must produce: one per declared entry, plus the SAP
    /// HANA component of the application's own SAP system when the registration names a
    /// System ID and no declaration already describes that component.
    fn expected(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (component_type, sid) in self.declared {
            let id = format!("{sid}-{component_type}");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if let Some(sid) = self.sid() {
            let id = format!("{sid}-HANA");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.sort();
        ids
    }

    /// The `Sid` the registration carries, when its application type requires one.
    fn sid(&self) -> Option<String> {
        self.request()
            .get("Sid")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The `ComponentId`s a `ListComponents` reply reports, sorted.
fn listed(reply: &Reply) -> Vec<String> {
    let mut ids: Vec<String> = reply
        .member("Components")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|component| {
            component
                .get("ComponentId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// `ListComponents` for `application_id`.
async fn list(emulator: &Emulator, application_id: &str) -> Reply {
    emulator
        .send_in_region(
            REGION,
            "/list-components",
            json!({"ApplicationId": application_id}),
        )
        .await
}

/// `GetApplication`'s `Components` for `application_id`, sorted.
async fn application_components(emulator: &Emulator, application_id: &str) -> Vec<String> {
    let reply = emulator
        .send_in_region(
            REGION,
            "/get-application",
            json!({"ApplicationId": application_id}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application_id:?}: expected 200 from GetApplication, got {reply:?}"
    );
    let mut ids = string_list(reply.member("Application").and_then(|app| app.get("Components")));
    ids.sort();
    ids
}

/// `GetComponent` for one component of `application_id`.
async fn get_component(emulator: &Emulator, application_id: &str, component_id: &str) -> Reply {
    emulator
        .send_in_region(
            REGION,
            "/get-component",
            json!({"ApplicationId": application_id, "ComponentId": component_id}),
        )
        .await
}

/// RULE: while a registration is still running its application has no component — the
/// listing answers 200 with none, `GetApplication` reports none, and `GetComponent` of a
/// component the registration described resolves to nothing — and once the discovery has
/// completed every reader reports exactly the components it produced.
#[tokio::test]
async fn components_appear_only_once_the_registration_has_discovered_them() {
    for case in CASES {
        let emulator = Emulator::new();
        let registered = emulator
            .send_in_region(REGION, "/register-application", case.request())
            .await;
        assert_eq!(
            registered.status, 200,
            "case {:?}: expected 200 from RegisterApplication, got {registered:?}",
            case.name
        );
        assert_eq!(
            registered.app_str("DiscoveryStatus"),
            "REGISTERING",
            "case {:?}: discovery runs while the registration does, got {registered:?}",
            case.name
        );
        assert_eq!(
            string_list(registered.app("Components")),
            Vec::<String>::new(),
            "case {:?}: a registration still running has discovered no component, got \
             {registered:?}",
            case.name
        );

        // Every reader, during the discovery window. Polling repeatedly is what a client
        // waiting for discovery does, and it must not change the answer.
        for poll in 0..3 {
            let during = list(&emulator, case.application_id).await;
            assert_eq!(
                during.status, 200,
                "case {:?} poll {poll}: an application whose discovery is running is \
                 listed, not refused, got {during:?}",
                case.name
            );
            assert_eq!(
                listed(&during),
                Vec::<String>::new(),
                "case {:?} poll {poll}: expected no component before discovery completed, \
                 got {during:?}",
                case.name
            );
        }
        assert_eq!(
            application_components(&emulator, case.application_id).await,
            Vec::<String>::new(),
            "case {:?}: GetApplication reports the components discovered, which is none \
             yet",
            case.name
        );
        for component_id in case.expected() {
            let refused = get_component(&emulator, case.application_id, &component_id).await;
            assert_eq!(
                refused.status, 400,
                "case {:?} at {component_id:?}: a component discovery has not produced \
                 resolves to nothing, got {refused:?}",
                case.name
            );
            assert_eq!(
                refused.code(),
                VALIDATION_EXCEPTION,
                "case {:?} at {component_id:?}: expected {VALIDATION_EXCEPTION}, got \
                 {refused:?}",
                case.name
            );
        }

        // The discovery the clock owes the registration completes.
        settle().await;

        let after = list(&emulator, case.application_id).await;
        assert_eq!(
            after.status, 200,
            "case {:?}: expected 200 from ListComponents, got {after:?}",
            case.name
        );
        assert_eq!(
            listed(&after),
            case.expected(),
            "case {:?}: expected the components discovery produced, got {after:?}",
            case.name
        );
        assert_eq!(
            application_components(&emulator, case.application_id).await,
            case.expected(),
            "case {:?}: GetApplication and ListComponents must agree",
            case.name
        );
        for component_id in case.expected() {
            let read = get_component(&emulator, case.application_id, &component_id).await;
            assert_eq!(
                read.status, 200,
                "case {:?} at {component_id:?}: a discovered component is readable, got \
                 {read:?}",
                case.name
            );
        }

        // They stay discovered: a second reading of the same application reports the same
        // set rather than one that grows or shrinks with the reads.
        let again = list(&emulator, case.application_id).await;
        assert_eq!(
            listed(&again),
            case.expected(),
            "case {:?}: two listings of a settled application must agree, got {again:?}",
            case.name
        );
    }
}

/// RULE: the discovery window belongs to the application whose registration is running, so
/// a Region holding one application mid-registration and one whose discovery completed
/// lists the components of the second and none of the first — and an `ApplicationId`
/// naming no registration is still refused as not found.
#[tokio::test]
async fn one_applications_discovery_window_does_not_hide_anothers_components() {
    let emulator = Emulator::new();
    let settled = &CASES[2];
    let registering = &CASES[1];

    let first = emulator
        .send_in_region(REGION, "/register-application", settled.request())
        .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from RegisterApplication, got {first:?}"
    );
    settle().await;

    let second = emulator
        .send_in_region(REGION, "/register-application", registering.request())
        .await;
    assert_eq!(
        second.status, 200,
        "expected 200 from RegisterApplication, got {second:?}"
    );

    // Narrowed to each application.
    assert_eq!(
        listed(&list(&emulator, settled.application_id).await),
        settled.expected(),
        "case {:?}: a settled application reports its discovered components",
        settled.name
    );
    assert_eq!(
        listed(&list(&emulator, registering.application_id).await),
        Vec::<String>::new(),
        "case {:?}: an application still registering reports none",
        registering.name
    );

    // Over the whole Region: only what has been discovered.
    let whole_region = emulator
        .send_in_region(REGION, "/list-components", json!({}))
        .await;
    assert_eq!(
        whole_region.status, 200,
        "expected 200 from ListComponents over the Region, got {whole_region:?}"
    );
    assert_eq!(
        listed(&whole_region),
        settled.expected(),
        "case \"the whole Region\": expected only the components discovery produced, got \
         {whole_region:?}"
    );

    // An id naming no registration here is a 404, whatever anyone's discovery is doing.
    let absent = list(&emulator, "discovery-never-registered").await;
    assert_eq!(
        absent.status, 404,
        "case \"an application nothing registered\": expected 404, got {absent:?}"
    );
}

/// RULE: a discovered component lives as long as its application, so one of a
/// deregistered application is gone with it — the listing for that id is refused as not
/// found once the deregistration the clock owes falls due.
#[tokio::test]
async fn discovered_components_end_with_their_application() {
    let emulator = Emulator::new();
    let case = &CASES[3];

    let registered = emulator
        .send_in_region(REGION, "/register-application", case.request())
        .await;
    assert_eq!(
        registered.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {registered:?}",
        case.name
    );
    settle().await;
    assert_eq!(
        listed(&list(&emulator, case.application_id).await),
        case.expected(),
        "case {:?}: expected the components discovery produced",
        case.name
    );

    let deregistered = emulator
        .send_in_region(
            REGION,
            "/deregister-application",
            json!({"ApplicationId": case.application_id}),
        )
        .await;
    assert_eq!(
        deregistered.status, 200,
        "case {:?}: expected 200 from DeregisterApplication, got {deregistered:?}",
        case.name
    );
    // While the deregistration runs the application still exists, so its discovered
    // components do too.
    assert_eq!(
        listed(&list(&emulator, case.application_id).await),
        case.expected(),
        "case {:?}: a deregistration under way leaves the components listed",
        case.name
    );

    settle().await;
    let gone = list(&emulator, case.application_id).await;
    assert_eq!(
        gone.status, 404,
        "case {:?}: the components of a deregistered application resolve to nothing, got \
         {gone:?}",
        case.name
    );
}

/// RULE: the rule holds over the model's whole `ComponentType` catalogue, not the types
/// one row happens to name: whichever type a registration declares, that component is
/// absent while discovery runs and present once it has completed.
#[tokio::test]
async fn every_modelled_component_type_is_discovered_rather_than_declared() {
    for component_type in COMPONENT_TYPE_VALUES {
        let emulator = Emulator::new();
        let application_id = "discovery-by-type";
        let mut body = valid_registration(application_id, "SAP_ABAP");
        let object: &mut Map<String, Value> =
            body.as_object_mut().expect("the body is a JSON object");
        object.insert(
            "ComponentsInfo".to_string(),
            json!([{
                "ComponentType": component_type,
                "Sid": "ABC",
                "Ec2InstanceId": INSTANCE,
            }]),
        );
        let registered = emulator
            .send_in_region(REGION, "/register-application", body)
            .await;
        assert_eq!(
            registered.status, 200,
            "case {component_type:?}: expected 200 from RegisterApplication, got \
             {registered:?}"
        );

        let during = list(&emulator, application_id).await;
        assert_eq!(
            listed(&during),
            Vec::<String>::new(),
            "case {component_type:?}: expected no component before discovery completed, got \
             {during:?}"
        );

        settle().await;
        let after = list(&emulator, application_id).await;
        assert_eq!(
            listed(&after),
            vec![format!("ABC-{component_type}")],
            "case {component_type:?}: expected the discovered component, got {after:?}"
        );
    }
}

/// RULE: the bound on how many components one application can have is the bound on what a
/// registration may declare, so a discovery produces at most that many — the five the
/// model's `ComponentInfoList` admits, plus the SAP HANA component of the application's
/// own SAP system.
#[tokio::test]
async fn a_discovery_produces_at_most_the_declared_components_and_the_systems_own() {
    let emulator = Emulator::new();
    let case = &CASES[3];
    let registered = emulator
        .send_in_region(REGION, "/register-application", case.request())
        .await;
    assert_eq!(
        registered.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {registered:?}",
        case.name
    );
    settle().await;

    let discovered = listed(&list(&emulator, case.application_id).await);
    assert_eq!(
        discovered.len(),
        MAX_DECLARED_COMPONENTS,
        "case {:?}: expected one component per declaration, got {discovered:?}",
        case.name
    );
    assert_eq!(
        discovered,
        case.expected(),
        "case {:?}: expected the declared components, got {discovered:?}",
        case.name
    );
}
