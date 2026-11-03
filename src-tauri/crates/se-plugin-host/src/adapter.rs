use crate::{
    abi_worker::{AbiWorker, CallKind},
    broker::PluginBroker,
    validation::{encode, error},
};
use cordis_core::{Context, InjectSpec, Plugin};
use se_plugin_protocol::*;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
pub(crate) struct Leases {
    pub live: AtomicBool,
    pub snapshots: Mutex<BTreeMap<String, ViewSnapshot>>,
}

pub(crate) struct AdapterInput {
    pub manifest: Manifest,
    pub generation: String,
    pub config: ConfigPayload,
    pub worker: Arc<AbiWorker>,
    pub broker: Arc<PluginBroker>,
    pub leases: Arc<Leases>,
    pub deadline: std::time::Instant,
}

pub(crate) struct CAbiPlugin;
impl Plugin for CAbiPlugin {
    type Config = AdapterInput;
    type Input = AdapterInput;
    type PrepareError = PluginError;
    type ApplyError = PluginError;

    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("termul.native-plugin.v1")
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::none().require("termul.plugin-broker.v1")
    }
    fn prepare(&self, input: Self::Config) -> Result<Self::Input, Self::PrepareError> {
        validate_manifest(&input.manifest)?;
        validate_config_payload(&input.manifest.contributes.configuration, &input.config)?;
        Ok(input)
    }
    async fn apply(&self, ctx: Context, input: &Self::Input) -> Result<(), Self::ApplyError> {
        crate::validation::CONTROL_DEADLINE
            .scope(input.deadline, activate(ctx, input))
            .await
    }
}
struct Admission<'a>(&'a PluginBroker, &'a str, &'a str, bool);
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if !self.3 {
            self.0.withdraw(self.1, self.2);
        }
    }
}
async fn activate(ctx: Context, input: &AdapterInput) -> Result<(), PluginError> {
    let mut admission = Admission(&input.broker, &input.manifest.id, &input.generation, false);
    let activation = ActivationPayload {
        schema_version: SCHEMA_VERSION,
        plugin_id: input.manifest.id.clone(),
        generation: input.generation.clone(),
        config: input.config.clone(),
    };
    input
        .broker
        .begin_activation(&input.generation, &input.manifest)?;
    input
        .worker
        .call(CallKind::Create, encode(&activation)?)
        .await?;
    // From the moment create succeeds, every path owns exactly one destroy obligation.
    let worker = input.worker.clone();
    if ctx
        .effect(move || async move {
            let _ = worker.call(CallKind::Destroy, Vec::new()).await;
        })
        .is_err()
    {
        let _ = input.worker.call(CallKind::Destroy, Vec::new()).await;
        return Err(error(
            PLUGIN_CALLBACK_ERROR,
            "cannot register native cleanup",
        ));
    }
    let leases = input.leases.clone();
    let broker = input.broker.clone();
    let id = input.manifest.id.clone();
    let generation = input.generation.clone();
    ctx.effect_sync(move || {
        leases.live.store(false, Ordering::Release);
        leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        broker.withdraw(&id, &generation);
    })
    .map_err(|_| error(PLUGIN_CALLBACK_ERROR, "cannot register host lease cleanup"))?;
    // No broker registrations or externally visible contributions before validation.
    for view in &input.manifest.contributes.views {
        let request = RenderRequest {
            schema_version: SCHEMA_VERSION,
            view_id: view.id.clone(),
        };
        let bytes = input
            .worker
            .call(CallKind::Render, encode(&request)?)
            .await?;
        let tree = crate::validation::tree(&bytes, &view.id)?;
        let snapshot = ViewSnapshot {
            plugin_id: input.manifest.id.clone(),
            generation: input.generation.clone(),
            revision: 1,
            config_revision: input.config.revision,
            tree,
        };
        input
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(view.id.clone(), snapshot);
    }
    input.broker.publish(
        input.manifest.id.clone(),
        input.generation.clone(),
        input.manifest.clone(),
        input.worker.clone(),
    )?;
    input.leases.live.store(true, Ordering::Release);
    admission.3 = true;
    Ok(())
}
