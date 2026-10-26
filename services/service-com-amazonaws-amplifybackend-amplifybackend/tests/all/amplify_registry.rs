// SPDX-License-Identifier: CC0-1.0

//! The Amplify app registry this service's app-identity gate reads, as a test fixture.
//!
//! An `AppId` is minted by Amplify's `CreateApp` and by nothing else, so
//! `crate::state::app_is_registered` answers from the Amplify service's own app store in
//! the shared engine. A suite that needs an app to EXIST therefore has to put one in that
//! store, and a suite that needs an id to name NO app simply leaves it out.
//!
//! [`register_amplify_app`] writes the record straight into that store rather than calling
//! Amplify's `CreateApp` over the wire, for two reasons: `CreateApp` mints the id itself,
//! so a suite could not name the app ids its own cases are written around, and its own
//! per-account app quota is Amplify's rule, not a rule of the fixture these suites need.
//! What AmplifyBackend reads of the record is its presence under the id, which is exactly
//! what this writes.
//!
//! The cross-service path through Amplify's real `CreateApp` is exercised in
//! `core/router/tests/all/amplifybackend_app_registration.rs`, where both services are
//! mounted on one engine.

use service_com_amazonaws_amplify_amplify::ops::types::App;
use service_com_amazonaws_amplify_amplify::state::{
    now, AmplifyApp, ComAmazonawsAmplifyAmplifyState,
};

/// The region the fixture's app ARNs name. Any region does: AmplifyBackend's gate reads
/// the app's presence, not its ARN.
const REGION: &str = "us-east-1";

/// Register an Amplify app under `app_id`, so AmplifyBackend's app-identity gate sees an
/// app that exists. Idempotent: registering the same id twice replaces the record.
pub fn register_amplify_app(engine: &engine::InMemoryEngine, app_id: &str, name: &str) {
    let created = now();
    let app = App {
        app_id: app_id.to_string(),
        app_arn: ::emulator_server::emulator_arn("amplify", REGION, &format!("apps/{app_id}")),
        name: name.to_string(),
        tags: None,
        description: String::new(),
        repository: String::new(),
        platform: "WEB".to_string(),
        create_time: created,
        update_time: created,
        compute_role_arn: None,
        iam_service_role_arn: None,
        environment_variables: std::collections::HashMap::new(),
        default_domain: format!("{app_id}.amplifyapp.com"),
        enable_branch_auto_build: false,
        enable_branch_auto_deletion: Some(false),
        enable_basic_auth: false,
        basic_auth_credentials: None,
        custom_rules: None,
        production_branch: None,
        build_spec: None,
        custom_headers: None,
        enable_auto_branch_creation: Some(false),
        auto_branch_creation_patterns: None,
        auto_branch_creation_config: None,
        repository_clone_method: None,
        cache_config: None,
        webhook_create_time: None,
        waf_configuration: None,
        job_config: None,
    };
    engine
        .state::<ComAmazonawsAmplifyAmplifyState>()
        .lock()
        .apps
        .insert(app_id.to_string(), AmplifyApp::new(app));
}
