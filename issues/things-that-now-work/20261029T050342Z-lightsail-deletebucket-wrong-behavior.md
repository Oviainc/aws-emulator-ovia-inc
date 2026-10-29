Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 060392db00c5b3260ef85f15c64a4c64c2352242

# Deleting a bucket leaves another bucket's accessLogConfig.destination naming a bucket the account no longer holds — the one reference the bucket removal does not release

_Filed: 2026-10-29T05:03:42Z_
_Operation: DeleteBucket | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `060392db00c5b3260ef85f15c64a4c64c2352242`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A bucket's access log is written into ANOTHER bucket of the same account and Region: `UpdateBucket` stores `accessLogConfig.destination` only after resolving it against the store, and refuses a destination that is not a bucket of this account in this Region. `DeleteBucket` never looks at that reference. It evaluates exactly the four force-delete conditions the model documents (objects, access keys, resources granted access, origin of a distribution) and then calls `store.remove_bucket(..)`, which removes the control-plane record alone — its own doc names only the object store as the other thing a removal has to release.

So after deleting a bucket that was some other bucket's log destination, the store holds — and `GetBuckets` reports, unconditionally, because `StoredBucket::view` always fills `accessLogConfig` — a bucket whose `accessLogConfig` is `{enabled: true, destination: "<deleted bucket>"}`: exactly the state `UpdateBucket` in this same crate refuses to create. The source bucket can no longer be re-saved either: re-sending its own current configuration through `UpdateBucket` is now refused with "is not a bucket of this account in {region}", so the record reports a configuration its own write path rejects.

This is the one reference kind the crate's removal walks do not cover. For an instance the crate states the rule explicitly — "Release every reference the store holds to the instance `name`" — and enumerates four kinds of holder, a bucket's `resourcesReceivingAccess` among them; the bucket→bucket access-log reference has no such release and no comment admitting the omission.

## Evidence

The write path refuses to CREATE this state — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_bucket.rs:301-306 and :314-321:
```
            let unknown_destination = input
                .access_log_config
                .as_ref()
                .and_then(|config| config.destination.as_ref())
                .filter(|destination| store.bucket(&region, destination).is_none())
                .cloned();
```
```
            if let Some(destination) = unknown_destination {
                return Err(invalid(format!(
                    "accessLogConfig.destination '{destination}' is not a bucket of this \
                     account in {region}; a bucket's access logs are saved in a Lightsail \
                     bucket in the same account and the same Amazon Web Services Region as \
                     the source bucket"
                )));
            }
```
The delete path never consults it — src/ops/delete_bucket.rs:188-204 reads only `access_keys`, `resources_receiving_access` and `distributions_with_origin`, and src/ops/delete_bucket.rs:211-243 evaluates only those four conditions:
```
        // "You must force delete the bucket if it has one of the following conditions: The
        // bucket is the origin of a distribution. The bucket has instances that were
        // granted access to it using the SetResourceAccessForBucket action. The bucket has
        // objects. The bucket has access keys." Each of those four is a condition this
        // store holds the records for, so each is evaluated here.
```
and the removal itself touches nothing else — src/state.rs:14303-14312:
```
    /// Remove the bucket `name` names in `region`, answering it. `None` when
    /// nothing matched.
    ///
    /// Only the control-plane RECORD: the bucket's objects live in this emulator's
    /// one object store, which [`release_bucket_object_store`] is what releases
    /// (it takes that store's lock, so it is called with no guard of this store
    /// held).
    pub fn remove_bucket(&mut self, region: &str, name: &str) -> Option<StoredBucket> {
        self.buckets.remove(&(region.to_string(), name.to_string()))
    }
```
(`grep -n 'access_log_config' src/` finds writers only in create_bucket.rs:305 and update_bucket.rs:341-342; nothing in any delete path.)

The stale reference is reported on every read — src/state.rs:2206-2234, `StoredBucket::view`, fills it unconditionally:
```
            access_log_config: Some(self.access_log_config.clone()),
```

The crate's own stated rule for a delete, which this one does not follow — src/state.rs:13659-13670:
```
    /// Release every reference the store holds to the instance `name`,
    /// answering one entry per release so the caller can log it.
    ///
    /// Four kinds of record can hold one: a block storage disk (`attachedTo`
    /// plus the device `path` it is mounted at), a static IP (`attachedTo`), a
    /// load balancer (the instance registered with it), and a bucket (the
    /// instance it reports in `resourcesReceivingAccess`, which "describe
    /// Lightsail instances that have access to the bucket").
```

Test coverage: tests/all/bucket_configuration_update.rs:456-467 pins both refusals for a destination that is not a bucket of this account in this Region ("a destination this account holds no bucket under", "a destination bucket of another Region"), and :251-252 pins the enabled form being stored; tests/all/bucket_lifecycle.rs:421-424 reads `accessLogConfig` back. No test deletes a destination bucket — `grep -n DeleteBucket tests/all/bucket_configuration_update.rs` returns nothing — so nothing pins the current behaviour and no comment states a reason for it.

## Steps to Reproduce

1. CreateBucket with bucketName="log-destination", bundleId="small_1_0" in us-east-1 — 200.
2. CreateBucket with bucketName="source", bundleId="small_1_0" in us-east-1 — 200.
3. UpdateBucket with bucketName="source", accessLogConfig={enabled:true, destination:"log-destination"} — 200; GetBuckets reports source's accessLogConfig naming "log-destination".
4. DeleteBucket with bucketName="log-destination" (it holds no object, no access key, no granted resource and is no distribution origin) — 200, the record is removed by src/ops/delete_bucket.rs with no look at any other bucket's accessLogConfig.
5. GetBuckets with bucketName="source": accessLogConfig is still {enabled:true, destination:"log-destination"}, while GetBuckets for "log-destination" answers the DoesNotExist envelope.
6. Re-send the source bucket's own current configuration: UpdateBucket with bucketName="source", accessLogConfig={enabled:true, destination:"log-destination"} — now refused 400 "accessLogConfig.destination 'log-destination' is not a bucket of this account in us-east-1", the very state step 5 still reports.
