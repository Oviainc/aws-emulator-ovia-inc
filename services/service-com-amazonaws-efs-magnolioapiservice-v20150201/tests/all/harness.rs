// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the emulator through: this one
//! service mounted on one shared engine behind the same `MultiServiceRouter` the
//! production entrypoint serves, plus one method per modelled route.
//!
//! It lives here, and not in a suite, so a new suite reuses the request builder, the
//! SigV4 scoping the handlers read the Region from, and the clock-settling helper
//! instead of copying them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;
use service_com_amazonaws_efs_magnolioapiservice_v20150201::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to `region` and to EFS's signing name, which
/// is what the router routes on and what the handlers read the Region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/elasticfilesystem/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// One HTTP answer, reduced to what the assertions read.
pub struct Reply {
    pub status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    pub error_type: Option<String>,
    pub body: Value,
    pub body_len: usize,
}

impl Reply {
    pub fn get(&self, key: &str) -> &Value {
        &self.body[key]
    }

    pub fn string(&self, key: &str) -> String {
        self.body[key].as_str().unwrap_or_default().to_string()
    }
}

/// The emulator under test: this one service mounted on one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    pub fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub fn in_region(region: &str) -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://elasticfilesystem.{}.amazonaws.com{path}",
                self.region
            ))
            .header("host", format!("elasticfilesystem.{}.amazonaws.com", self.region))
            .header("authorization", authorization(&self.region));
        let bytes = match &body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(serde_json::to_vec(value).expect("a serializable body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = builder.body(bytes).expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = response.into_body();
        Reply {
            status,
            error_type,
            body: serde_json::from_slice(&payload).unwrap_or(Value::Null),
            body_len: payload.len(),
        }
    }

    pub async fn create_file_system(&self, body: Value) -> Reply {
        self.send("POST", "/2015-02-01/file-systems", Some(body)).await
    }

    pub async fn describe_file_systems(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/2015-02-01/file-systems".to_string()
        } else {
            format!("/2015-02-01/file-systems?{query}")
        };
        self.send("GET", &path, None).await
    }

    pub async fn delete_file_system(&self, id: &str) -> Reply {
        self.send("DELETE", &format!("/2015-02-01/file-systems/{id}"), None)
            .await
    }

    pub async fn create_mount_target(&self, body: Value) -> Reply {
        self.send("POST", "/2015-02-01/mount-targets", Some(body)).await
    }

    pub async fn describe_mount_targets(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/2015-02-01/mount-targets".to_string()
        } else {
            format!("/2015-02-01/mount-targets?{query}")
        };
        self.send("GET", &path, None).await
    }

    pub async fn delete_mount_target(&self, id: &str) -> Reply {
        self.send("DELETE", &format!("/2015-02-01/mount-targets/{id}"), None)
            .await
    }

    /// `DELETE /2015-02-01/access-points/{AccessPointId}`, the modelled
    /// `DeleteAccessPoint` binding. The label takes an id or an access-point ARN, so the
    /// value is percent-encoded nowhere: both forms are sent as the caller wrote them.
    pub async fn delete_access_point(&self, id: &str) -> Reply {
        self.send("DELETE", &format!("/2015-02-01/access-points/{id}"), None)
            .await
    }

    pub async fn update_file_system_protection(&self, id: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/2015-02-01/file-systems/{id}/protection"),
            Some(body),
        )
        .await
    }

    /// `GET /2015-02-01/file-systems/{FileSystemId}/backup-policy`, the modelled
    /// `DescribeBackupPolicy` binding.
    pub async fn describe_backup_policy(&self, id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/2015-02-01/file-systems/{id}/backup-policy"),
            None,
        )
        .await
    }

    /// `PUT /2015-02-01/file-systems/{FileSystemId}/backup-policy`, the modelled
    /// `PutBackupPolicy` binding.
    pub async fn put_backup_policy(&self, id: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/2015-02-01/file-systems/{id}/backup-policy"),
            Some(body),
        )
        .await
    }

    /// `GET /2015-02-01/file-systems/{FileSystemId}/policy`, the modelled
    /// `DescribeFileSystemPolicy` binding.
    pub async fn describe_file_system_policy(&self, id: &str) -> Reply {
        self.send("GET", &format!("/2015-02-01/file-systems/{id}/policy"), None)
            .await
    }

    /// `PUT /2015-02-01/file-systems/{FileSystemId}/policy`, the modelled
    /// `PutFileSystemPolicy` binding.
    pub async fn put_file_system_policy(&self, id: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/2015-02-01/file-systems/{id}/policy"),
            Some(body),
        )
        .await
    }

    /// `DELETE /2015-02-01/file-systems/{FileSystemId}/policy`, the modelled
    /// `DeleteFileSystemPolicy` binding.
    pub async fn delete_file_system_policy(&self, id: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/2015-02-01/file-systems/{id}/policy"),
            None,
        )
        .await
    }

    pub async fn create_replication_configuration(&self, source_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/2015-02-01/file-systems/{source_id}/replication-configuration"),
            Some(body),
        )
        .await
    }

    pub async fn describe_replication_configurations(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/2015-02-01/file-systems/replication-configurations".to_string()
        } else {
            format!("/2015-02-01/file-systems/replication-configurations?{query}")
        };
        self.send("GET", &path, None).await
    }

    pub async fn delete_replication_configuration(&self, source_id: &str, query: &str) -> Reply {
        let path = format!("/2015-02-01/file-systems/{source_id}/replication-configuration");
        let path = if query.is_empty() {
            path
        } else {
            format!("{path}?{query}")
        };
        self.send("DELETE", &path, None).await
    }

    /// Create one `available` file system, waiting out the clock-owed
    /// `creating` -> `available` transition so a mount target or a replication
    /// configuration can be added to it.
    pub async fn available_file_system(&self, token: &str) -> String {
        let created = self
            .create_file_system(json!({ "CreationToken": token }))
            .await;
        assert_eq!(
            created.status, 201,
            "creating file system {token}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.string("FileSystemId");
        settle().await;
        id
    }
}

/// Wait past the clock-owed `creating` -> `available` transition. The handlers never
/// sleep for it — they store the due instant — so only a test that wants the settled
/// state waits here.
pub async fn settle() {
    tokio::time::sleep(state::CREATING_FOR + Duration::from_millis(150)).await;
}

/// Wait past the clock-owed removal of a resource an accepted delete moved to
/// `deleting`. Like [`settle`], the handlers store the due instant instead of sleeping
/// for it, so only a test that wants the completed deletion waits here.
pub async fn settle_deletion() {
    tokio::time::sleep(state::DELETING_FOR + Duration::from_millis(150)).await;
}

/// `^fs-[0-9a-f]{8,40}$` / `^fsmt-[0-9a-f]{8,40}$`, the modelled id patterns.
pub fn matches_id_pattern(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix) {
        Some(hex) => {
            (8..=40).contains(&hex.len())
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        None => false,
    }
}
