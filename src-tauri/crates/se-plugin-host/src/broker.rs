//! The one compile-time cordis service; dynamic service names stay in this table.
//!
//! Publication is a commit operation. The caller must validate the entire enabled
//! graph with `order` before starting activation, and publish only after create and
//! initial views have succeeded. Withdrawal closes admission without calling FFI;
//! the lifecycle owner then tears down dependents in reverse topological order.

use crate::abi_worker::{AbiWorker, CallKind};
use se_plugin_protocol::{
    parse_service_reply, validate_manifest, validate_service_request, Manifest, PluginError,
    ServiceRequest, PLUGIN_INVALID_MANIFEST, PLUGIN_INVALID_REQUEST, PLUGIN_PENDING,
    PLUGIN_REENTRANT, PLUGIN_SERVICE_CYCLE, PLUGIN_SERVICE_UNAVAILABLE, PLUGIN_STALE_GENERATION,
    PLUGIN_TIMEOUT,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_CHAIN_DEPTH: usize = 8;

struct Registration {
    generation: String,
    manifest: Manifest,
    worker: Arc<AbiWorker>,
}

#[derive(Default)]
struct Registrations {
    plugins: BTreeMap<String, Registration>,
    // Withdrawal immediately blocks transitive dependents, before their teardown.
    admitted: BTreeSet<String>,
    activating: BTreeMap<String, (String, Manifest)>,
}

#[derive(Default)]
pub struct PluginBroker {
    registrations: Mutex<Registrations>,
}

impl cordis_core::Service for PluginBroker {
    const NAME: &'static str = "termul.plugin-broker.v1";
}

impl PluginBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the whole desired-enabled graph, then return only resolvable IDs.
    ///
    /// All v1 requirements are mandatory: the protocol has no optional flag.
    /// Missing dependencies (and their consumers) are pending, not graph errors.
    /// Duplicates and cycles are errors even in a currently unresolvable subgraph.
    /// Independent ready nodes are ordered lexicographically for reproducibility.
    pub fn order(manifests: &[Manifest]) -> Result<Vec<String>, PluginError> {
        let mut plugins = BTreeMap::new();
        let mut providers = BTreeMap::new();
        for manifest in manifests {
            validate_manifest(manifest)?;
            if plugins.insert(manifest.id.as_str(), manifest).is_some() {
                return Err(PluginError::new(
                    PLUGIN_INVALID_MANIFEST,
                    "duplicate plugin id in dependency graph",
                ));
            }
            for service in &manifest.services.provides {
                if providers
                    .insert(
                        (service.name.as_str(), service.version),
                        manifest.id.as_str(),
                    )
                    .is_some()
                {
                    return Err(PluginError::new(
                        PLUGIN_INVALID_MANIFEST,
                        "duplicate service provider in dependency graph",
                    ));
                }
            }
        }

        let mut dependencies = BTreeMap::new();
        let mut consumers: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut missing = BTreeSet::new();
        for (&id, manifest) in &plugins {
            let mut required = BTreeSet::new();
            for service in &manifest.services.requires {
                if let Some(&provider) = providers.get(&(service.name.as_str(), service.version)) {
                    required.insert(provider);
                    consumers.entry(provider).or_default().insert(id);
                } else {
                    missing.insert(id);
                }
            }
            dependencies.insert(id, required);
        }

        // Detect cycles over *all* nodes, not only the currently resolvable set.
        let mut indegree: BTreeMap<_, _> = dependencies
            .iter()
            .map(|(&id, required)| (id, required.len()))
            .collect();
        let mut ready: BTreeSet<_> = indegree
            .iter()
            .filter_map(|(&id, &degree)| (degree == 0).then_some(id))
            .collect();
        let mut sorted = Vec::with_capacity(plugins.len());
        while let Some(id) = ready.pop_first() {
            sorted.push(id);
            if let Some(dependents) = consumers.get(id) {
                for dependent in dependents {
                    if let Some(degree) = indegree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            ready.insert(*dependent);
                        }
                    }
                }
            }
        }
        if sorted.len() != plugins.len() {
            return Err(PluginError::new(
                PLUGIN_SERVICE_CYCLE,
                "cyclic plugin service dependencies",
            ));
        }

        let mut resolved = BTreeSet::new();
        let mut result = Vec::new();
        for id in sorted {
            if !missing.contains(id)
                && dependencies[id]
                    .iter()
                    .all(|provider| resolved.contains(provider))
            {
                resolved.insert(id);
                result.push(id.to_owned());
            }
        }
        Ok(result)
    }

    /// Authorize outbound calls during create/render without publishing providers.
    pub(crate) fn begin_activation(
        &self,
        generation: &str,
        manifest: &Manifest,
    ) -> Result<(), PluginError> {
        validate_manifest(manifest)?;
        let mut state = self.registrations.lock().map_err(|_| unavailable())?;
        state.activating.insert(
            manifest.id.clone(),
            (generation.to_owned(), manifest.clone()),
        );
        Ok(())
    }

    /// Atomically expose a fully committed generation. Never replaces a live ID.
    pub fn publish(
        &self,
        id: String,
        generation: String,
        manifest: Manifest,
        worker: Arc<AbiWorker>,
    ) -> Result<(), PluginError> {
        if manifest.id != id || generation.is_empty() {
            return Err(PluginError::new(
                PLUGIN_INVALID_MANIFEST,
                "broker registration identity mismatch",
            ));
        }
        let mut state = self.registrations.lock().map_err(|_| unavailable())?;
        if state.plugins.contains_key(&id) {
            return Err(PluginError::new(
                PLUGIN_INVALID_MANIFEST,
                "plugin generation already published; withdraw it before replacement",
            ));
        }
        let mut manifests: Vec<_> = state
            .plugins
            .values()
            .map(|registration| registration.manifest.clone())
            .collect();
        manifests.push(manifest.clone());
        let admitted: BTreeSet<_> = Self::order(&manifests)?.into_iter().collect();
        if !admitted.contains(&id) {
            return Err(PluginError::new(
                PLUGIN_PENDING,
                "plugin service dependencies are not active",
            ));
        }
        // No mutation until the entire candidate graph has passed validation.
        state.activating.remove(&id);
        state.plugins.insert(
            id,
            Registration {
                generation,
                manifest,
                worker,
            },
        );
        state.admitted = admitted;
        Ok(())
    }

    /// Close routing synchronously, ignoring stale cleanup from an older lease.
    /// Already admitted calls own their worker Arc and may finish until deadline.
    pub fn withdraw(&self, id: &str, generation: &str) {
        let removed = {
            let mut state = self
                .registrations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .activating
                .get(id)
                .is_some_and(|(active_generation, _)| active_generation == generation)
            {
                state.activating.remove(id);
            }
            if state
                .plugins
                .get(id)
                .is_none_or(|registration| registration.generation != generation)
            {
                return;
            }
            let removed = state.plugins.remove(id);
            let manifests: Vec<_> = state
                .plugins
                .values()
                .map(|registration| registration.manifest.clone())
                .collect();
            // Removal cannot introduce duplicates/cycles. Fail closed if the
            // invariant is ever broken, including after a poisoned lock.
            state.admitted = Self::order(&manifests)
                .unwrap_or_default()
                .into_iter()
                .collect();
            removed
        };
        // Even destruction of the channel handle stays outside the routing lock.
        drop(removed);
    }

    /// Active plugin leases, including registrations contributing no services.
    pub fn count(&self) -> usize {
        self.registrations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .plugins
            .len()
    }

    /// Host-only callback routing. `chain` is the full path ending in caller;
    /// an empty path is accepted for an outermost callback and seeded here.
    pub fn call(
        &self,
        caller: &str,
        generation: &str,
        request: ServiceRequest,
        deadline: Instant,
        chain: Vec<String>,
    ) -> Result<Vec<u8>, PluginError> {
        validate_service_request(&request)?;
        // The binding plan is narrower than the current protocol validator.
        if !(1..=128).contains(&request.method.chars().count()) {
            return Err(PluginError::new(
                PLUGIN_INVALID_REQUEST,
                "service method must contain 1 to 128 characters",
            ));
        }
        if Instant::now() >= deadline {
            return Err(PluginError::new(
                PLUGIN_TIMEOUT,
                "service deadline exceeded",
            ));
        }
        let input = serde_json::to_vec(&request)
            .map_err(|_| PluginError::new(PLUGIN_INVALID_REQUEST, "invalid service request"))?;
        let (worker, chain) = {
            let state = self.registrations.lock().map_err(|_| unavailable())?;
            let (source_generation, source_manifest, admitted) =
                if let Some(source) = state.plugins.get(caller) {
                    (
                        &source.generation,
                        &source.manifest,
                        state.admitted.contains(caller),
                    )
                } else if let Some((generation, manifest)) = state.activating.get(caller) {
                    (generation, manifest, true)
                } else {
                    return Err(unavailable());
                };
            if source_generation != generation {
                return Err(PluginError::new(
                    PLUGIN_STALE_GENERATION,
                    "caller generation is no longer active",
                ));
            }
            if !admitted
                || !source_manifest.services.requires.iter().any(|service| {
                    service.name == request.service && service.version == request.version
                })
            {
                return Err(unavailable());
            }
            let (provider_id, provider) = state
                .plugins
                .iter()
                .find(|(id, registration)| {
                    state.admitted.contains(*id)
                        && registration
                            .manifest
                            .services
                            .provides
                            .iter()
                            .any(|service| {
                                service.name == request.service
                                    && service.version == request.version
                            })
                })
                .ok_or_else(unavailable)?;
            let chain = extend_chain(caller, provider_id, chain)?;
            (Arc::clone(&provider.worker), chain)
        };
        // In particular, nested HostApi broker callbacks can acquire the lock.
        let output = worker.call_blocking(CallKind::Service, input, deadline, chain)?;
        parse_service_reply(&output)?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_plugin_protocol::{
        AbiRange, ConfigurationSchema, Contributions, ServiceDeclaration, Services,
    };

    fn manifest(id: &str, provides: &[(&str, u32)], requires: &[(&str, u32)]) -> Manifest {
        let declarations = |services: &[(&str, u32)]| {
            services
                .iter()
                .map(|(name, version)| ServiceDeclaration {
                    name: (*name).to_owned(),
                    version: *version,
                })
                .collect()
        };
        Manifest {
            schema_version: 1,
            id: id.to_owned(),
            name: id.to_owned(),
            version: "1.0.0".to_owned(),
            abi: AbiRange { min: 1, max: 1 },
            contributes: Contributions {
                commands: vec![],
                views: vec![],
                configuration: ConfigurationSchema {
                    title: "Settings".to_owned(),
                    properties: BTreeMap::new(),
                },
            },
            services: Services {
                provides: declarations(provides),
                requires: declarations(requires),
            },
        }
    }

    #[test]
    fn empty_graph_and_new_broker_have_no_registrations() {
        assert!(PluginBroker::order(&[]).unwrap().is_empty());
        let broker = PluginBroker::new();
        assert_eq!(broker.count(), 0);
        broker.withdraw("test.missing", "old-generation");
        assert_eq!(broker.count(), 0);
        assert_eq!(
            <PluginBroker as cordis_core::Service>::NAME,
            "termul.plugin-broker.v1"
        );
    }

    #[test]
    fn graph_order_is_deterministic_provider_before_consumer() {
        let a = manifest("test.a", &[("test.service-a", 1)], &[]);
        let b = manifest("test.b", &[("test.service-b", 1)], &[("test.service-a", 1)]);
        let c = manifest(
            "test.c",
            &[],
            &[("test.service-a", 1), ("test.service-b", 1)],
        );
        let independent = manifest("test.independent", &[], &[]);
        let expected = vec!["test.a", "test.b", "test.c", "test.independent"];
        assert_eq!(
            PluginBroker::order(&[c.clone(), independent.clone(), b.clone(), a.clone()]).unwrap(),
            expected
        );
        assert_eq!(
            PluginBroker::order(&[a, b, c, independent]).unwrap(),
            expected
        );
    }

    #[test]
    fn multiple_services_from_same_provider_are_one_graph_edge() {
        let provider = manifest("test.provider", &[("test.one", 1), ("test.two", 1)], &[]);
        let consumer = manifest("test.consumer", &[], &[("test.one", 1), ("test.two", 1)]);
        assert_eq!(
            PluginBroker::order(&[consumer, provider]).unwrap(),
            vec!["test.provider", "test.consumer"]
        );
    }

    #[test]
    fn duplicate_providers_reject_entire_graph_even_when_pending() {
        let a = manifest("test.a", &[("test.service", 1)], &[("test.missing", 1)]);
        let b = manifest("test.b", &[("test.service", 1)], &[]);
        let graph = vec![a.clone(), b];
        let original = graph.clone();
        assert_eq!(
            PluginBroker::order(&graph).unwrap_err().code,
            PLUGIN_INVALID_MANIFEST
        );
        assert_eq!(graph, original);
        let duplicate_within_manifest = manifest(
            "test.duplicate",
            &[("test.service", 1), ("test.service", 1)],
            &[],
        );
        assert_eq!(
            PluginBroker::order(&[duplicate_within_manifest])
                .unwrap_err()
                .code,
            PLUGIN_INVALID_MANIFEST
        );
        assert_eq!(
            PluginBroker::order(&[a.clone(), a]).unwrap_err().code,
            PLUGIN_INVALID_MANIFEST
        );
    }

    #[test]
    fn service_version_is_part_of_identity() {
        let one = manifest("test.one", &[("test.service", 1)], &[]);
        let two = manifest("test.two", &[("test.service", 2)], &[]);
        let consumer = manifest("test.consumer", &[], &[("test.service", 2)]);
        assert_eq!(
            PluginBroker::order(&[consumer, two, one]).unwrap(),
            vec!["test.one", "test.two", "test.consumer"]
        );
    }

    #[test]
    fn cycles_are_rejected_even_with_another_missing_dependency() {
        let a = manifest(
            "test.a",
            &[("test.one", 1)],
            &[("test.two", 1), ("test.missing", 1)],
        );
        let b = manifest("test.b", &[("test.two", 1)], &[("test.one", 1)]);
        assert_eq!(
            PluginBroker::order(&[a, b]).unwrap_err().code,
            PLUGIN_SERVICE_CYCLE
        );
        let self_cycle = manifest("test.self", &[("test.service", 1)], &[("test.service", 1)]);
        assert_eq!(
            PluginBroker::order(&[self_cycle]).unwrap_err().code,
            PLUGIN_SERVICE_CYCLE
        );
    }

    #[test]
    fn missing_dependency_excludes_consumer_transitively_until_provider_arrives() {
        let a = manifest("test.a", &[("test.one", 1)], &[("test.missing", 2)]);
        let b = manifest("test.b", &[], &[("test.one", 1)]);
        let unrelated = manifest("test.unrelated", &[], &[]);
        let wrong_version = manifest("test.wrong-version", &[("test.missing", 1)], &[]);
        let mut graph = vec![a, b, unrelated, wrong_version];
        assert_eq!(
            PluginBroker::order(&graph).unwrap(),
            vec!["test.unrelated", "test.wrong-version"]
        );
        graph.push(manifest("test.provider", &[("test.missing", 2)], &[]));
        assert_eq!(
            PluginBroker::order(&graph).unwrap(),
            vec![
                "test.provider",
                "test.a",
                "test.b",
                "test.unrelated",
                "test.wrong-version"
            ]
        );
    }

    #[test]
    fn graph_rejects_invalid_manifests_before_ordering() {
        let mut invalid = manifest("test.valid", &[], &[]);
        invalid.schema_version = 2;
        assert_eq!(
            PluginBroker::order(&[invalid]).unwrap_err().code,
            PLUGIN_INVALID_MANIFEST
        );
    }

    #[test]
    fn service_chain_rejects_self_reentry_duplicates_and_wrong_tail() {
        assert_eq!(
            extend_chain("test.a", "test.b", vec![]).unwrap(),
            vec!["test.a", "test.b"]
        );
        for (caller, provider, chain) in [
            ("test.a", "test.a", vec![]),
            ("test.b", "test.a", vec!["test.a".into(), "test.b".into()]),
            (
                "test.b",
                "test.c",
                vec!["test.a".into(), "test.a".into(), "test.b".into()],
            ),
            ("test.b", "test.c", vec!["test.a".into()]),
        ] {
            assert_eq!(
                extend_chain(caller, provider, chain).unwrap_err().code,
                PLUGIN_REENTRANT
            );
        }
    }

    #[test]
    fn service_chain_allows_eight_nodes_but_not_nine() {
        let chain: Vec<_> = (0..7).map(|i| format!("test.node{i}")).collect();
        let eight = extend_chain("test.node6", "test.node7", chain).unwrap();
        assert_eq!(eight.len(), MAX_CHAIN_DEPTH);
        assert_eq!(
            extend_chain("test.node7", "test.node8", eight)
                .unwrap_err()
                .code,
            PLUGIN_REENTRANT
        );
    }
}

fn unavailable() -> PluginError {
    PluginError::new(PLUGIN_SERVICE_UNAVAILABLE, "plugin service is unavailable")
}

fn extend_chain(
    caller: &str,
    provider: &str,
    mut chain: Vec<String>,
) -> Result<Vec<String>, PluginError> {
    if chain.is_empty() {
        chain.push(caller.to_owned());
    }
    let unique: BTreeSet<_> = chain.iter().collect();
    if chain.last().map(String::as_str) != Some(caller)
        || unique.len() != chain.len()
        || chain.iter().any(|id| id == provider)
        || chain.len() >= MAX_CHAIN_DEPTH
    {
        return Err(PluginError::new(
            PLUGIN_REENTRANT,
            "reentrant, invalid, or excessively deep service call chain",
        ));
    }
    chain.push(provider.to_owned());
    Ok(chain)
}
