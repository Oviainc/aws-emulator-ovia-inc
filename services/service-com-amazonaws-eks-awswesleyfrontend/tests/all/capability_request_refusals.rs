// SPDX-License-Identifier: CC0-1.0

//! `CreateCapability` words the two refusals it decides from the request body the
//! way Amazon EKS does, and accepts every value the documented form allows.
//!
//! The RULES under test:
//!
//! * THE NAME'S FORM. "A unique name for the capability. The name must be unique
//!   within your cluster and can contain alphanumeric characters, hyphens, and
//!   underscores" (`CreateCapability$capabilityName`) — the member carries no
//!   `@pattern`, so the handler is what refuses a value outside that form, with the
//!   one fixed sentence that states the whole rule
//!   (`crate::state::INVALID_CAPABILITY_NAME_MESSAGE`). The sentence echoes nothing:
//!   the caller sent the name, and what it has to act on is the form.
//! * THE TYPE AND THE CONFIGURATION. "The structure of this object varies depending
//!   on the capability type" (`CapabilityConfigurationRequest`), so a configuration
//!   member that belongs to another `CapabilityType` than the one the request names
//!   is refused `InvalidParameterException` with the one fixed sentence that names
//!   the mismatched PAIR (`crate::state::CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE`),
//!   whichever member of the configuration was the mismatched one.
//!
//! The rows differ along what each rule generalises over: for the name, which
//! character class the value breaks the form with and where (a leading hyphen or
//! underscore, an inner space, punctuation, a dot) beside names of every allowed
//! shape, which are accepted; for the configuration, which member was supplied
//! against which of the three declared types, including the type that takes no
//! configuration at all — with the two matching pairs as the control.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`. With no IAM mounted beside this service, `roleArn` is a
//! member no refusal can be grounded in (`peer_resolution_is_grounded` pins that
//! rule), so these rows reach the body validation this suite is about.

use service_com_amazonaws_eks_awswesleyfrontend::ops::types::CAPABILITY_TYPE_VALUES;
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE, CLUSTER_ACTIVATION_DELAY,
    INVALID_CAPABILITY_NAME_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{create_cluster, settle, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// A role of the account this emulator serves, so the pass-role authorization —
/// which needs no peer — is never what answers.
const CAPABILITY_ROLE: &str = "arn:aws:iam::000000000000:role/eksCapabilityRole";

async fn create_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/capabilities/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

/// The smallest `CreateCapability` body, with the name and the type left to the row.
fn capability_body(name: &str, capability_type: &str) -> serde_json::Value {
    serde_json::json!({
        "capabilityName": name,
        "type": capability_type,
        "roleArn": CAPABILITY_ROLE,
        "deletePropagationPolicy": "RETAIN",
    })
}

/// An `argoCd` configuration whose one required member
/// (`ArgoCdConfigRequest$awsIdc.idcInstanceArn`) is set, so a row is judged on the
/// type/configuration rule rather than on a missing member.
fn argo_cd_configuration() -> serde_json::Value {
    serde_json::json!({
        "argoCd": {
            "awsIdc": { "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef" },
        },
    })
}

/// An `ack` configuration, the other member of `CapabilityConfigurationRequest`.
fn ack_configuration() -> serde_json::Value {
    serde_json::json!({ "ack": { "enableCrossNamespace": true } })
}

/// Stand up an `ACTIVE` cluster for each name, with ONE wait for the clock-owed
/// `CREATING` -> `ACTIVE` move.
async fn active_clusters(router: &MultiServiceRouter, clusters: &[&str]) {
    for cluster in clusters {
        let created = create_cluster(router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateCluster for {cluster:?}, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;
}

// ---------------------------------------------------------------------------
// RULE: a `capabilityName` outside the documented form is refused 400
// `InvalidParameterException` with the one fixed sentence that states the form;
// every name inside it is accepted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_capability_name_outside_the_documented_form_is_refused_with_one_sentence() {
    let router = emulator();
    let refusals_cluster = "capability-name-refused";
    let accepted_cluster = "capability-name-accepted";
    active_clusters(&router, &[refusals_cluster, accepted_cluster]).await;

    // Rows outside the form: where the offending character sits and which class it
    // belongs to — a leading hyphen and a leading underscore (the form starts with
    // a letter or a number), an inner space, punctuation, a dot, a path separator,
    // and a letter outside ASCII.
    let refused = [
        ("a leading hyphen", "-leading-hyphen"),
        ("a leading underscore", "_leading_underscore"),
        ("an inner space", "order cap"),
        ("trailing punctuation", "cap!!"),
        ("a dot, which no capability name carries", "cap.one"),
        ("a path separator", "cap/one"),
        ("a non-ASCII letter", "capabilité"),
    ];
    for (case, name) in refused {
        let answer = create_capability(&router, refusals_cluster, &capability_body(name, "ACK"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from CreateCapability for {name:?}, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException for {name:?}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            INVALID_CAPABILITY_NAME_MESSAGE,
            "case {case:?}: expected the fixed capability-name sentence, got {:?}",
            answer.message()
        );
        let read = describe_capability(&router, refusals_cluster, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store nothing, got {read:?}"
        );
    }

    // Rows inside the form, one per declared `CapabilityType` because "you can only
    // create one Capability of each type on a given Amazon EKS cluster": a name of
    // letters and a hyphen, one starting with a digit, and one carrying an
    // underscore and mixed case.
    let accepted = ["valid-name", "9starts-with-a-digit", "Mixed_Case_1"];
    assert_eq!(
        accepted.len(),
        CAPABILITY_TYPE_VALUES.len(),
        "the accepted rows must fit one capability type each, got {} names for {} types",
        accepted.len(),
        CAPABILITY_TYPE_VALUES.len()
    );
    for (name, capability_type) in accepted.iter().zip(CAPABILITY_TYPE_VALUES) {
        let case = format!("{name:?} as a {capability_type} capability");
        let answer =
            create_capability(&router, accepted_cluster, &capability_body(name, capability_type))
                .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: a name inside the documented form must be accepted, got {answer:?} \
             ({})",
            answer.message()
        );
        let read = describe_capability(&router, accepted_cluster, name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the created capability must read back, got {read:?}"
        );
        assert_eq!(
            read.body["capability"]["capabilityName"],
            serde_json::json!(name),
            "case {case}: the capability read back must carry the name it was given, got {:?}",
            read.body["capability"]["capabilityName"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `configuration` member that belongs to another `CapabilityType` than the
// one the request names is refused 400 `InvalidParameterException` with the one
// fixed sentence that names the mismatched pair.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_configuration_of_another_capability_type_is_refused_with_one_sentence() {
    let router = emulator();
    let refusals_cluster = "capability-config-refused";
    let accepted_cluster = "capability-config-accepted";
    active_clusters(&router, &[refusals_cluster, accepted_cluster]).await;

    // Every (type, configuration member) pair the two do not belong together in,
    // including `KRO`, the declared type that takes no configuration at all.
    let refused = [
        ("an Argo CD configuration on an ACK capability", "ACK", argo_cd_configuration()),
        ("an ACK configuration on an Argo CD capability", "ARGOCD", ack_configuration()),
        ("an Argo CD configuration on a KRO capability", "KRO", argo_cd_configuration()),
        ("an ACK configuration on a KRO capability", "KRO", ack_configuration()),
    ];
    for (index, (case, capability_type, configuration)) in refused.into_iter().enumerate() {
        let name = format!("config-mismatch-{index}");
        let mut body = capability_body(&name, capability_type);
        body["configuration"] = configuration;
        let answer = create_capability(&router, refusals_cluster, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 from CreateCapability, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE,
            "case {case:?}: expected the fixed type/configuration sentence, got {:?}",
            answer.message()
        );
        let read = describe_capability(&router, refusals_cluster, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store nothing, got {read:?}"
        );
    }

    // The control: each configuration member against the type it belongs to.
    let accepted = [
        ("an ACK configuration on an ACK capability", "ACK", ack_configuration()),
        (
            "an Argo CD configuration on an Argo CD capability",
            "ARGOCD",
            argo_cd_configuration(),
        ),
    ];
    for (index, (case, capability_type, configuration)) in accepted.into_iter().enumerate() {
        let name = format!("config-match-{index}");
        let mut body = capability_body(&name, capability_type);
        body["configuration"] = configuration;
        let answer = create_capability(&router, accepted_cluster, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a configuration of the capability's own type must be accepted, \
             got {answer:?} ({})",
            answer.message()
        );
        let read = describe_capability(&router, accepted_cluster, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created capability must read back, got {read:?}"
        );
        assert_eq!(
            read.body["capability"]["type"],
            serde_json::json!(capability_type),
            "case {case:?}: the capability read back must carry the type it was given, got {:?}",
            read.body["capability"]["type"]
        );
    }
}
