// SPDX-License-Identifier: CC0-1.0

//! What a registration DERIVES rather than echoes, read back once discovery reports
//! success.
//!
//! RULE: registering an SAP application records it in AWS Service Catalog AppRegistry and
//! discovers the components of the SAP system it registered, so the application a later
//! read reports carries state no request member carried — an AppRegistry application ARN
//! of the Region it was registered in, which `GetApplication` also resolves that one
//! application by, and a non-empty component set, every id of which resolves one by one —
//! whether or not the registration described a component itself.
//!
//! Every call goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: derived state is only
//! believed when `GetApplication` / `GetComponent` reports it back.
//!
//! The table spans what the rule generalises over: both `ApplicationType` values, the
//! `ApplicationId` forms the modelled `^[\w\d\.-]+$` admits, the System ID, both forms the
//! modelled `InstanceId` admits (`^i-[\w\d]{8}$` and `^i-[\w\d]{17}$`), how many components
//! the registration described itself (none at all — the registration the issue records —
//! one, one that describes the very component the System ID does, and the five the
//! model's `ComponentInfoList` admits), and the Region the registration was made in. The
//! sibling suites pin the halves: `tests/all/app_registry_arn.rs` the assigned ARN on its
//! own, `tests/all/component_cross_references.rs` the component set on its own; this suite
//! pins that ONE settled read carries both.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_registry_arn::{app_registry_arn_prefix, is_app_registry_arn, reported};
use crate::application_lifecycle::{settle, string_list, valid_registration, Emulator, Reply};

/// One registration, and the state the service must derive for it.
#[derive(Debug)]
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    /// The regional endpoint the registration is made at, which is the Region its derived
    /// AppRegistry application belongs to.
    region: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `Sid` the registration carries: the SAP system it registers.
    sid: &'static str,
    /// The `SapInstanceNumber` the registration carries.
    sap_instance_number: &'static str,
    /// The single EC2 instance the registration names (`InstanceList` is
    /// `@length(min: 1, max: 1)`).
    instance: &'static str,
    /// The `ComponentsInfo` entries, as (`ComponentType`, `Sid`, `Ec2InstanceId`); the
    /// types come from the model's own `ComponentType` catalogue
    /// (`smithy_query(query="shape", shape="ComponentType")`: HANA, HANA_NODE, ABAP, ASCS,
    /// DIALOG, WEBDISP, WD, ERS).
    components_info: &'static [(&'static str, &'static str, &'static str)],
    /// The `ComponentId`s the settled application must report, in order: one per described
    /// component, plus the SAP HANA component of the SAP system the `Sid` names when
    /// nothing described that one.
    expected_components: &'static [&'static str],
}

/// The rows: one registration each.
const CASES: &[Case] = &[
    // The registration the issue records: the members a HANA application requires and no
    // `ComponentsInfo` at all, so everything the read reports beyond them is derived.
    Case {
        name: "the recorded HANA registration, describing no component",
        region: "us-east-1",
        application_id: "HDB",
        application_type: "HANA",
        sid: "HDB",
        sap_instance_number: "00",
        instance: "i-1234567890abcdef0",
        components_info: &[],
        expected_components: &["HDB-HANA"],
    },
    // The same shape of registration in another Region, on the SHORT instance-id form, so
    // neither the derived ARN's Region nor the derived component depends on the first row's
    // Region or instance-id form.
    Case {
        name: "a HANA registration in another Region on the short instance-id form",
        region: "eu-west-1",
        application_id: "prd.hana.01",
        application_type: "HANA",
        sid: "PRD",
        sap_instance_number: "01",
        instance: "i-abcd1234",
        components_info: &[],
        expected_components: &["PRD-HANA"],
    },
    // A registration that described ONE component of another type of its own system: the
    // described component and the derived one are both reported.
    Case {
        name: "a HANA registration describing one component of its own system",
        region: "ap-southeast-2",
        application_id: "QAS_NODES",
        application_type: "HANA",
        sid: "QAS",
        sap_instance_number: "02",
        instance: "i-0123456789abcdef0",
        components_info: &[("HANA_NODE", "QAS", "i-0123456789abcdef1")],
        expected_components: &["QAS-HANA_NODE", "QAS-HANA"],
    },
    // A registration that described the very component the System ID names: one component,
    // not two, so the derivation adds nothing a registration already described.
    Case {
        name: "a HANA registration describing the component its System ID names",
        region: "us-east-1",
        application_id: "hdb-declared",
        application_type: "HANA",
        sid: "DEC",
        sap_instance_number: "03",
        instance: "i-0123456789abcdef0",
        components_info: &[("HANA", "DEC", "i-0123456789abcdef2")],
        expected_components: &["DEC-HANA"],
    },
    // The other application type, describing nothing. The component derived for either
    // type is the SAP HANA component of the SAP system the registration's own `Sid` names
    // — the component this service attributes the application's SAP HANA databases to
    // (`service_com_amazonaws_ssmsap_ssmsap::state::registered_components` and
    // `StoredApplication::databases`, whose agreement
    // `tests/all/component_cross_references.rs` pins); this row only holds the derivation
    // to the same rule for an ABAP registration as for a HANA one.
    Case {
        name: "an ABAP registration describing no component",
        region: "us-east-1",
        application_id: "abap-plain",
        application_type: "SAP_ABAP",
        sid: "ABC",
        sap_instance_number: "04",
        instance: "i-9876543210fedcba0",
        components_info: &[],
        expected_components: &["ABC-HANA"],
    },
    // The largest `ComponentsInfo` the model admits (`ComponentInfoList` is
    // `@length(min: 0, max: 5)`), beside the derived component: the most components one
    // registration can leave behind.
    Case {
        name: "an ABAP registration describing the largest ComponentsInfo the model admits",
        region: "eu-west-1",
        application_id: "abap-five-described",
        application_type: "SAP_ABAP",
        sid: "ZZ9",
        sap_instance_number: "42",
        instance: "i-0123456789abcdef8",
        components_info: &[
            ("ABAP", "ZZ9", "i-0123456789abcdef3"),
            ("ASCS", "ZZ9", "i-0123456789abcdef4"),
            ("ERS", "ZZ9", "i-0123456789abcdef5"),
            ("DIALOG", "ZZ9", "i-0123456789abcdef6"),
            ("WD", "ZZ9", "i-0123456789abcdef7"),
        ],
        expected_components: &[
            "ZZ9-ABAP",
            "ZZ9-ASCS",
            "ZZ9-ERS",
            "ZZ9-DIALOG",
            "ZZ9-WD",
            "ZZ9-HANA",
        ],
    },
];

/// The `RegisterApplication` body for a row: the smallest accepted registration for its
/// type, with the row's System ID, instance number, instance and described components.
fn request(case: &Case) -> Value {
    let mut body = valid_registration(case.application_id, case.application_type);
    let object = body
        .as_object_mut()
        .expect("a registration body is an object");
    object.insert("Sid".to_string(), json!(case.sid));
    object.insert(
        "SapInstanceNumber".to_string(),
        json!(case.sap_instance_number),
    );
    object.insert("Instances".to_string(), json!([case.instance]));
    if !case.components_info.is_empty() {
        object.insert(
            "ComponentsInfo".to_string(),
            Value::Array(
                case.components_info
                    .iter()
                    .map(|(component_type, sid, instance)| {
                        json!({
                            "ComponentType": component_type,
                            "Sid": sid,
                            "Ec2InstanceId": instance,
                        })
                    })
                    .collect(),
            ),
        );
    }
    body
}

/// The `Components` of the `Application` a reply reports, as strings — read through the
/// one list reader the harness owns.
fn components(reply: &Reply) -> Vec<String> {
    string_list(reply.app("Components"))
}

/// The expected `ComponentId`s of a row, as owned strings.
fn expected(case: &Case) -> Vec<String> {
    case.expected_components
        .iter()
        .map(|id| (*id).to_string())
        .collect()
}

/// `GetApplication` at the row's regional endpoint, naming the application by `reference`.
async fn get_application(emulator: &Emulator, case: &Case, reference: Value) -> Reply {
    emulator
        .send_in_region(case.region, "/get-application", reference)
        .await
}

/// RULE: a registration derives BOTH the AppRegistry application that records it and the
/// components of the SAP system it registered, so the application a settled read reports
/// carries an assigned ARN of its Region and a non-empty component set — each id of which
/// `GetComponent` resolves — however little the registration itself described.
#[tokio::test]
async fn a_settled_registration_reports_its_derived_app_registry_arn_and_components() {
    let emulator = Emulator::new();
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    let mut minted: Vec<String> = Vec::new();

    for case in CASES {
        let name = case.name;
        let registered = emulator
            .send_in_region(case.region, "/register-application", request(case))
            .await;
        assert_eq!(
            registered.status, 200,
            "case {name:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        // Discovery is the asynchronous phase the registration starts, so the application
        // is reported in it before the clock brings it to success.
        assert_eq!(
            registered.app_str("DiscoveryStatus"),
            "REGISTERING",
            "case {name:?}: expected the registering discovery status, got {registered:?}"
        );

        let arn = reported(&registered).unwrap_or_else(|| {
            panic!("case {name:?}: RegisterApplication must report an AppRegistryArn, got {registered:?}")
        });
        assert!(
            is_app_registry_arn(&arn),
            "case {name:?}: expected an ARN of the modelled AppRegistryArn form, got {arn:?}"
        );
        assert!(
            arn.starts_with(&app_registry_arn_prefix(case.region)),
            "case {name:?}: expected an AppRegistry application ARN of this account and Region, got {arn:?}"
        );
        assert!(
            assigned.insert(arn.clone()),
            "case {name:?}: every registration records its OWN AppRegistry application, got the already-assigned {arn:?}"
        );
        minted.push(arn);

        // Discovery is what produces the components, and it is the asynchronous phase
        // this registration has only just started, so its own reply reports none yet.
        assert_eq!(
            components(&registered),
            Vec::<String>::new(),
            "case {name:?}: a registration still running has discovered no component, got {registered:?}"
        );
    }

    settle().await;

    for (case, arn) in CASES.iter().zip(&minted) {
        let name = case.name;
        let read = get_application(
            &emulator,
            case,
            json!({"ApplicationId": case.application_id}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: expected 200 from GetApplication, got {read:?}"
        );
        assert_eq!(
            read.app_str("DiscoveryStatus"),
            "SUCCESS",
            "case {name:?}: expected a succeeded discovery, got {read:?}"
        );
        assert_eq!(
            read.app_str("Status"),
            "ACTIVATED",
            "case {name:?}: expected the settled status, got {read:?}"
        );
        assert_eq!(
            reported(&read).as_deref(),
            Some(arn.as_str()),
            "case {name:?}: a settled application reports the ARN its registration was assigned, got {read:?}"
        );

        let reported_components = components(&read);
        assert!(
            !reported_components.is_empty(),
            "case {name:?}: a succeeded discovery reports the components it discovered, got {read:?}"
        );
        assert_eq!(
            reported_components,
            expected(case),
            "case {name:?}: expected the derived components, got {read:?}"
        );

        // A component the application reports is a component it HAS: following the id into
        // `GetComponent` reads that component of that application.
        for component_id in &reported_components {
            let component = emulator
                .send_in_region(
                    case.region,
                    "/get-component",
                    json!({
                        "ApplicationId": case.application_id,
                        "ComponentId": component_id,
                    }),
                )
                .await;
            assert_eq!(
                component.status, 200,
                "case {name:?} at {component_id:?}: expected 200 from GetComponent, got {component:?}"
            );
            let member = |name: &str| {
                component
                    .member("Component")
                    .and_then(|value| value.get(name))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            assert_eq!(
                member("ComponentId"),
                *component_id,
                "case {name:?} at {component_id:?}: expected that component back, got {component:?}"
            );
            assert_eq!(
                member("ApplicationId"),
                case.application_id,
                "case {name:?} at {component_id:?}: expected the component of that application, got {component:?}"
            );
        }

        // Reading derives nothing further: the second read reports exactly the first.
        let again = get_application(
            &emulator,
            case,
            json!({"ApplicationId": case.application_id}),
        )
        .await;
        assert_eq!(
            again.body, read.body,
            "case {name:?}: a second read reports the same derived state, got {again:?} vs {read:?}"
        );

        // The conjunction the two derivations make: the ARN the registration assigned is
        // an identifier of that one application, so a read carrying it as its only member
        // answers the very application — derived components and all — that the
        // `ApplicationId` answers.
        let by_registry = get_application(&emulator, case, json!({"AppRegistryArn": arn})).await;
        assert_eq!(
            by_registry.status, 200,
            "case {name:?}: expected 200 resolving by {arn:?}, got {by_registry:?}"
        );
        assert_eq!(
            by_registry.app_str("Id"),
            case.application_id,
            "case {name:?}: resolving by {arn:?} must answer that application, got {by_registry:?}"
        );
        assert_eq!(
            components(&by_registry),
            expected(case),
            "case {name:?}: the application resolved by its AppRegistry ARN reports its derived components, got {by_registry:?}"
        );
        assert_eq!(
            by_registry.body, read.body,
            "case {name:?}: the AppRegistryArn lookup must answer the id lookup's application, got {by_registry:?} vs {read:?}"
        );
    }
}
