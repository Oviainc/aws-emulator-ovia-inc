// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice service.
//!
//! EC2 Instance Connect pushes a short-lived SSH public key at an EC2 instance, so every
//! answer either operation gives is computed from four things kept here:
//!
//! * [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::instances`] — the
//!   instance registry: whether an instance exists, which Availability Zone it is in, what
//!   lifecycle state it is in, and whether its instance type supports the serial console.
//!   This service models no operation that launches an instance (it has exactly two, both
//!   key pushes), so the registry is seeded (see [`seed_instances`]) and is the single place
//!   an EC2 emulator with `RunInstances` would publish instances for this service to read.
//! * `pushed_keys` / `serial_console_keys` — the keys that were pushed, each with the
//!   instant it expires. An expired key is treated as absent on lookup.
//! * `serial_console_access_enabled` — the account-level serial-console toggle that
//!   `SerialConsoleAccessDisabledException` reports.
//! * `requests` — a bounded log of the calls served, so a caller can tell a recorded call
//!   from a canned reply.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a pushed key stays usable.
///
/// 60 seconds, from this service's own model: `SendSSHPublicKey` documents "The key remains
/// for 60 seconds." and `SendSerialConsoleSSHPublicKey` "The key remains for 60 seconds,
/// which gives you 60 seconds to establish a serial console connection to the instance
/// using SSH." — a duration the service documents, so it keeps that value.
pub const PUSHED_KEY_TTL: Duration = Duration::from_secs(60);

/// The only serial port the model admits (`SerialPort`, range 0..0, default 0 — "Currently
/// only port 0 is supported").
pub const SERIAL_CONSOLE_PORT: i32 = 0;

/// How many served calls [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::requests`]
/// keeps. The log exists so a test can see that a call was recorded and that request ids do
/// not repeat; it is bounded because a long-lived emulator must not grow a list forever.
pub const MAX_RECORDED_REQUESTS: usize = 256;

/// An instance's lifecycle state, as EC2 names `instance-state-name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceState {
    /// Launching; not yet reachable.
    Pending,
    /// Running — the only state a key can be pushed to.
    Running,
    /// On its way down.
    Stopping,
    /// Stopped.
    Stopped,
    /// Terminated.
    Terminated,
}

/// Why an instance in a given state cannot be connected to.
///
/// Both operations model both outcomes, and each maps this to its own error variant, so the
/// state-to-outcome rule lives in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectBlocked {
    /// The instance exists but has not finished launching: the modelled
    /// `EC2InstanceUnavailableException` ("The instance is currently unavailable. Wait a few
    /// minutes and try again.").
    NotYetAvailable,
    /// The instance is shutting down, stopped or terminated: the modelled
    /// `EC2InstanceStateInvalidException` ("Connecting to a stopped or terminated instance is
    /// not supported.").
    InvalidState,
}

impl InstanceState {
    /// `None` when a key may be pushed to an instance in this state, else why it may not.
    pub fn blocks_connect(self) -> Option<ConnectBlocked> {
        match self {
            Self::Running => None,
            Self::Pending => Some(ConnectBlocked::NotYetAvailable),
            Self::Stopping | Self::Stopped | Self::Terminated => Some(ConnectBlocked::InvalidState),
        }
    }

    /// The name EC2 gives this state, for an error message that says what was found.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Terminated => "terminated",
        }
    }
}

/// A registered EC2 instance: everything this service needs to decide a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The instance id (`i-...`) the registry is keyed by.
    pub instance_id: String,
    /// The Availability Zone the instance was launched in; a request that names a different
    /// one is refused.
    pub availability_zone: String,
    /// The instance type, reported in the serial-console refusal.
    pub instance_type: String,
    /// The lifecycle state.
    pub state: InstanceState,
    /// Whether this instance type supports the EC2 serial console ("Only Nitro instance
    /// types are currently supported", per `EC2InstanceTypeInvalidException`).
    pub serial_console_supported: bool,
}

impl Instance {
    /// A registry entry.
    pub fn new(
        instance_id: impl Into<String>,
        availability_zone: impl Into<String>,
        instance_type: impl Into<String>,
        state: InstanceState,
        serial_console_supported: bool,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            availability_zone: availability_zone.into(),
            instance_type: instance_type.into(),
            state,
            serial_console_supported,
        }
    }

    /// The same entry in another lifecycle state (a stop, a start, a termination).
    pub fn in_state(mut self, state: InstanceState) -> Self {
        self.state = state;
        self
    }
}

/// A key that was pushed, and when it stops being usable.
#[derive(Debug, Clone)]
pub struct PushedKey {
    /// The public key material exactly as the caller sent it.
    pub public_key: String,
    /// The OS user the key authenticates, or `None` for a serial-console push — the serial
    /// console takes no OS user, so its keys are keyed by port instead.
    pub os_user: Option<String>,
    /// When the push was served.
    pub pushed_at: Instant,
    /// `pushed_at` plus [`PUSHED_KEY_TTL`]; from this instant the key is gone.
    pub expires_at: Instant,
    /// The id of the request that pushed it.
    pub request_id: String,
}

impl PushedKey {
    fn new(
        public_key: &str,
        os_user: Option<&str>,
        request_id: &str,
        now: Instant,
    ) -> Self {
        Self {
            public_key: public_key.to_string(),
            os_user: os_user.map(str::to_string),
            pushed_at: now,
            expires_at: now + PUSHED_KEY_TTL,
            request_id: request_id.to_string(),
        }
    }

    /// Whether the key's lifetime has run out at `now`.
    pub fn is_expired_at(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    /// How much of the key's lifetime is left at `now` (zero once expired).
    pub fn remaining_at(&self, now: Instant) -> Duration {
        self.expires_at.saturating_duration_since(now)
    }
}

/// One served call: enough to show the call happened and which id it answered with.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// The `RequestId` returned to the caller.
    pub request_id: String,
    /// The operation name.
    pub operation: String,
    /// The instance the call named.
    pub instance_id: String,
    /// When it was served.
    pub at: Instant,
}

/// Per-service state for com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    /// The instance registry, keyed by instance id.
    pub instances: HashMap<String, Instance>,
    /// Keys pushed by `SendSSHPublicKey`, keyed by (instance id, OS user).
    pub pushed_keys: HashMap<(String, String), PushedKey>,
    /// Keys pushed by `SendSerialConsoleSSHPublicKey`, keyed by (instance id, serial port).
    pub serial_console_keys: HashMap<(String, i32), PushedKey>,
    /// The account-level EC2 serial console toggle (EC2's `EnableSerialConsoleAccess` /
    /// `DisableSerialConsoleAccess`). Enabled here so the happy path works; the
    /// serial-console refusal reads it.
    pub serial_console_access_enabled: bool,
    /// The most recent served calls, newest last, at most [`MAX_RECORDED_REQUESTS`].
    pub requests: Vec<RequestRecord>,
    /// A seed drawn once per process, so two runs of the emulator never answer with the same
    /// `RequestId` values.
    request_id_seed: u64,
    /// How many calls this store has answered; never repeats, so no `RequestId` does.
    request_sequence: u64,
}

/// A value that differs per process, from the seed the OS gives std's [`RandomState`].
fn process_seed() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u8(0);
    hasher.finish()
}

/// The instances this service starts with.
///
/// This service models no operation that launches, stops or terminates an instance, so
/// without a seed the registry would be empty and every push would answer 404 — the registry
/// could not decide that any instance exists. The fleet below is therefore a fixture standing
/// in for EC2's own `RunInstances`: four instances that between them reach every branch the
/// two operations have (running Nitro, stopped, running but not serial-console capable,
/// terminated). A test registers its own instances with
/// [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::register_instance`],
/// and when the EC2 emulator grows an instance store this seed is what it replaces.
///
/// The zone names use `us-east-1`, the region the emulator writes into the SDK shared config
/// it hands clients (`vendor/server/src/lib.rs`, `region = us-east-1`).
pub fn seed_instances() -> Vec<Instance> {
    vec![
        // Nitro, running: both operations succeed against this one.
        Instance::new(
            "i-0a1b2c3d4e5f60718",
            "us-east-1a",
            "m5.large",
            InstanceState::Running,
            true,
        ),
        // Stopped: `EC2InstanceStateInvalidException`.
        Instance::new(
            "i-0b2c3d4e5f6071829",
            "us-east-1b",
            "m5.large",
            InstanceState::Stopped,
            true,
        ),
        // Running, but a pre-Nitro instance type: the serial console refuses it with
        // `EC2InstanceTypeInvalidException` while `SendSSHPublicKey` still succeeds.
        Instance::new(
            "i-0c3d4e5f607182930",
            "us-east-1c",
            "m3.medium",
            InstanceState::Running,
            false,
        ),
        // Terminated: `EC2InstanceStateInvalidException`.
        Instance::new(
            "i-0d4e5f60718293041",
            "us-east-1a",
            "m5.large",
            InstanceState::Terminated,
            true,
        ),
    ]
}

impl Default for ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    fn default() -> Self {
        let mut state = Self {
            instances: HashMap::new(),
            pushed_keys: HashMap::new(),
            serial_console_keys: HashMap::new(),
            serial_console_access_enabled: true,
            requests: Vec::new(),
            request_id_seed: process_seed(),
            request_sequence: 0,
        };
        for instance in seed_instances() {
            state.register_instance(instance);
        }
        state
    }
}

impl ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    /// A fresh `RequestId` for a call about to be served.
    ///
    /// AWS answers both operations with an opaque UUID-shaped id, one per call. This builds
    /// one by hashing (`sha2`) the per-process seed, the per-call sequence number and the
    /// wall clock, and rendering the first 128 bits as hex (`hex`) in the 8-4-4-4-12 form a
    /// client's UUID parser accepts. The sequence number never repeats, so neither does an
    /// id within a run; the seed differs per process, so neither do ids between runs.
    pub fn next_request_id(&mut self) -> String {
        use sha2::Digest;

        self.request_sequence = self.request_sequence.wrapping_add(1);
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        let digest = sha2::Sha256::digest(format!(
            "{:016x}:{:016x}:{:032x}",
            self.request_id_seed, self.request_sequence, since_epoch
        ));
        let rendered = hex::encode(&digest[..16]);
        format!(
            "{}-{}-{}-{}-{}",
            &rendered[0..8],
            &rendered[8..12],
            &rendered[12..16],
            &rendered[16..20],
            &rendered[20..32]
        )
    }

    /// Add (or replace) a registry entry, returning the entry it displaced.
    pub fn register_instance(&mut self, instance: Instance) -> Option<Instance> {
        self.instances.insert(instance.instance_id.clone(), instance)
    }

    /// The registered instance with this id, if the registry holds one.
    pub fn instance(&self, instance_id: &str) -> Option<&Instance> {
        self.instances.get(instance_id)
    }

    /// Move a registered instance to another lifecycle state; `false` when it is unknown.
    pub fn set_instance_state(&mut self, instance_id: &str, state: InstanceState) -> bool {
        match self.instances.get_mut(instance_id) {
            Some(instance) => {
                instance.state = state;
                true
            }
            None => false,
        }
    }

    /// Store a key against (instance, OS user), replacing any key already there and
    /// restarting its lifetime at `now`. Returns the stored record.
    pub fn push_ssh_key(
        &mut self,
        instance_id: &str,
        os_user: &str,
        public_key: &str,
        request_id: &str,
        now: Instant,
    ) -> PushedKey {
        self.drop_expired_keys(now);
        let pushed = PushedKey::new(public_key, Some(os_user), request_id, now);
        self.pushed_keys.insert(
            (instance_id.to_string(), os_user.to_string()),
            pushed.clone(),
        );
        pushed
    }

    /// Store a serial-console key against (instance, serial port), replacing any key already
    /// there and restarting its lifetime at `now`. Returns the stored record.
    pub fn push_serial_console_key(
        &mut self,
        instance_id: &str,
        serial_port: i32,
        public_key: &str,
        request_id: &str,
        now: Instant,
    ) -> PushedKey {
        self.drop_expired_keys(now);
        let pushed = PushedKey::new(public_key, None, request_id, now);
        self.serial_console_keys
            .insert((instance_id.to_string(), serial_port), pushed.clone());
        pushed
    }

    /// The key usable for (instance, OS user) at `now`. An expired key reads as absent,
    /// whether or not anything has pruned it yet.
    pub fn active_ssh_key(
        &self,
        instance_id: &str,
        os_user: &str,
        now: Instant,
    ) -> Option<&PushedKey> {
        self.pushed_keys
            .get(&(instance_id.to_string(), os_user.to_string()))
            .filter(|key| !key.is_expired_at(now))
    }

    /// The serial-console key usable for (instance, port) at `now`, expiry applied as above.
    pub fn active_serial_console_key(
        &self,
        instance_id: &str,
        serial_port: i32,
        now: Instant,
    ) -> Option<&PushedKey> {
        self.serial_console_keys
            .get(&(instance_id.to_string(), serial_port))
            .filter(|key| !key.is_expired_at(now))
    }

    /// Forget every key whose lifetime has run out by `now`.
    ///
    /// This only materialises what the clock already made due — the keys were unusable from
    /// `expires_at` whether or not anyone called — so it runs on the write path, and reads
    /// ([`Self::active_ssh_key`]) apply the same cutoff without touching the maps.
    pub fn drop_expired_keys(&mut self, now: Instant) {
        self.pushed_keys.retain(|_, key| !key.is_expired_at(now));
        self.serial_console_keys
            .retain(|_, key| !key.is_expired_at(now));
    }

    /// Append a served call to the bounded request log.
    pub fn record_request(
        &mut self,
        request_id: &str,
        operation: &str,
        instance_id: &str,
        now: Instant,
    ) {
        if self.requests.len() >= MAX_RECORDED_REQUESTS {
            let overflow = self.requests.len() + 1 - MAX_RECORDED_REQUESTS;
            self.requests.drain(..overflow);
        }
        self.requests.push(RequestRecord {
            request_id: request_id.to_string(),
            operation: operation.to_string(),
            instance_id: instance_id.to_string(),
            at: now,
        });
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState>)
}
