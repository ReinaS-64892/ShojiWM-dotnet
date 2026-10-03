use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    host::InProcessDotNetHost,
    native::{
        BridgeError, EvaluateCachedRequest, EvaluateCandidatePreviewRequest,
        EvaluatePreviewRequest, EvaluateRequest, EvaluationContext, EvaluationResponse,
        InvokeHandlerRequest,
    },
};
use shojiwm_lib::runtime_api::{RuntimeConfigDelta, RuntimeHost};
use shojiwm_lib::runtime_input::RuntimeInputDeviceSnapshot;
use shojiwm_lib::ssd::{
    DecorationCachedEvaluationResult, DecorationEvaluationError, DecorationEvaluationResult,
    DecorationEvaluator, DecorationHandlerInvocation, DecorationTree, ManagedWindowState,
    WaylandOutputSnapshot, WaylandWindowSnapshot, WindowTransform,
};

/// Managed bridge reusing upstream's public snapshot, tree and reply types.
#[derive(Debug, Clone)]
pub struct DotNetDecorationEvaluator {
    component: PathBuf,
    config: PathBuf,
    state: Arc<Mutex<RuntimeState>>,
    pending: Option<Arc<PendingAssembly>>,
    // Drops after pending cleanup and the state/host, so config dependencies
    // remain available during abort/dispose as well as ordinary shutdown.
    generation: Option<Arc<super::assembly::GenerationDirectory>>,
    runtime_host: RuntimeHost,
}

/// A prepared candidate must be aborted when validation, a newer save or event
/// delivery cancels it. The lease holds no old generation/assembly directory.
#[derive(Debug)]
struct PendingAssembly {
    state: Arc<Mutex<RuntimeState>>,
    completed: AtomicBool,
}

impl Drop for PendingAssembly {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::Acquire) {
            let owner = DotNetDecorationEvaluator {
                component: PathBuf::new(),
                config: PathBuf::new(),
                state: self.state.clone(),
                generation: None,
                pending: None,
                runtime_host: RuntimeHost::detached(),
            };
            if let Ok(mut state) = owner.lock() {
                if let Err(error) = owner.use_host(&mut state, |host| host.abort_assembly()) {
                    tracing::warn!(%error, "failed to abort prepared C# assembly");
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct RuntimeState {
    host: Option<InProcessDotNetHost>,
    failure: Option<String>,
    displays: BTreeMap<String, WaylandOutputSnapshot>,
    inputs: BTreeMap<String, RuntimeInputDeviceSnapshot>,
    windows: BTreeMap<String, (WaylandWindowSnapshot, DecorationEvaluationResult)>,
}

impl RuntimeState {
    fn fail_host(&mut self, error: String) {
        self.failure = Some(format!("{error}; ShojiWM restart required"));
    }
}

// InProcessDotNetHost owns RAII destruction on the managed host thread.
// CLR/bootstrap remain loaded for process lifetime.

impl DotNetDecorationEvaluator {
    pub fn new(component: PathBuf, config: PathBuf) -> Self {
        Self {
            component,
            config,
            state: Arc::new(Mutex::new(RuntimeState::default())),
            generation: None,
            pending: None,
            runtime_host: RuntimeHost::detached(),
        }
    }

    pub fn with_host(mut self, host: RuntimeHost) -> Self {
        self.runtime_host = host;
        self
    }

    pub(crate) fn for_generation(
        component: PathBuf,
        config: PathBuf,
        directory: Arc<super::assembly::GenerationDirectory>,
    ) -> Self {
        let mut evaluator = Self::new(component, config);
        evaluator.generation = Some(directory);
        evaluator
    }

    pub(crate) fn has_active_host(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.failure.is_none() && state.host.is_some())
    }

    pub(crate) fn prepare_assembly(
        &self,
        config: PathBuf,
        directory: Arc<super::assembly::GenerationDirectory>,
    ) -> Result<Self, DecorationEvaluationError> {
        let mut next = self.clone();
        next.config = config;
        next.generation = Some(directory);
        next.pending = Some(Arc::new(PendingAssembly {
            state: self.state.clone(),
            completed: AtomicBool::new(false),
        }));
        {
            let mut state = self.lock()?;
            let path = next.config.to_str().ok_or_else(|| {
                DecorationEvaluationError::RuntimeProtocol("config path must be UTF-8".into())
            })?;
            self.use_host(&mut state, |host| host.prepare_assembly(path.to_owned()))?;
        }
        Ok(next)
    }

    /// Called exclusively within ConfigRuntime::reload, after all candidate
    /// previews validate. Retains the managed thread and permanent bootstrap.
    pub(crate) fn activate_prepared(&self) -> Result<(), DecorationEvaluationError> {
        if let Some(pending) = &self.pending {
            if pending.completed.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut state = self.lock()?;
            let committed = self.use_host(&mut state, |host| host.commit_assembly())?;
            self.publish_debug(committed.debug_config);
            state.windows.clear();
            pending.completed.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub(crate) fn validate_candidate(
        &self,
        snapshot: &WaylandWindowSnapshot,
    ) -> Result<(), DecorationEvaluationError> {
        if self.pending.is_some() {
            self.render_candidate_preview(snapshot, 0).map(|_| ())
        } else {
            self.render_preview(snapshot, 0).map(|_| ())
        }
    }

    pub(crate) fn window_snapshots(
        &self,
    ) -> Result<Vec<WaylandWindowSnapshot>, DecorationEvaluationError> {
        Ok(self
            .lock()?
            .windows
            .values()
            .map(|(snapshot, _)| snapshot.clone())
            .collect())
    }

    /// Dispose the managed host on its original thread. User cleanup can block;
    /// there is deliberately no unsafe thread termination or CLR restart.
    pub(crate) fn retire(&self, _reason: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.host.take();
            state.windows.clear();
            state.failure = Some("runtime host retired; ShojiWM restart required".into());
        }
    }

    pub fn set_display_state(&self, displays: BTreeMap<String, WaylandOutputSnapshot>) {
        if let Ok(mut state) = self.state.lock() {
            state.displays = displays;
        }
    }

    pub fn set_input_state(&self, inputs: BTreeMap<String, RuntimeInputDeviceSnapshot>) {
        if let Ok(mut state) = self.state.lock() {
            state.inputs = inputs;
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RuntimeState>, DecorationEvaluationError> {
        self.state.lock().map_err(|_| {
            DecorationEvaluationError::RuntimeProtocol(".NET runtime mutex poisoned".into())
        })
    }

    // Shared host lifetime/error mechanics, never an operation discriminator.
    fn use_host<T>(
        &self,
        state: &mut RuntimeState,
        call: impl FnOnce(&mut InProcessDotNetHost) -> Result<T, BridgeError>,
    ) -> Result<T, DecorationEvaluationError> {
        if let Some(error) = &state.failure {
            return Err(DecorationEvaluationError::RuntimeProtocol(error.clone()));
        }
        if state.host.is_none() {
            match InProcessDotNetHost::start(&self.component, &self.config) {
                Ok(host) => state.host = Some(host),
                Err(error) => {
                    state.fail_host(error.clone());
                    return Err(DecorationEvaluationError::RuntimeProtocol(error));
                }
            }
        }
        let result = call(state.host.as_mut().expect("host initialized above"));
        match result {
            Ok(value) => Ok(value),
            Err(BridgeError::Semantic(error)) => {
                Err(DecorationEvaluationError::RuntimeProtocol(error))
            }
            Err(BridgeError::Host(error)) => {
                state.fail_host(error.clone());
                Err(DecorationEvaluationError::RuntimeProtocol(error))
            }
        }
    }
    fn context(state: &RuntimeState, now_ms: u64) -> EvaluationContext {
        EvaluationContext {
            now_ms,
            displays: state.displays.clone(),
            inputs: state.inputs.clone(),
        }
    }
    fn publish_debug(
        &self,
        debug_config: Option<shojiwm_lib::runtime_debug::RuntimeDebugConfigUpdate>,
    ) {
        RuntimeConfigDelta {
            debug_config,
            ..Default::default()
        }
        .publish(&self.runtime_host);
    }
    pub fn preload(&self) -> Result<(), DecorationEvaluationError> {
        let mut state = self.lock()?;
        self.use_host(&mut state, |host| host.drain_preload())
    }
    pub fn lifecycle_enable(
        &self,
        reason: &str,
    ) -> Result<DecorationHandlerInvocation, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let response =
            self.use_host(&mut state, |host| host.lifecycle_enable(reason.to_owned()))?;
        self.publish_debug(response.debug_config);
        Ok(DecorationHandlerInvocation {
            actions: response.actions,
            ..Default::default()
        })
    }
    fn evaluation_result(
        response: EvaluationResponse,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        let node = Self::decode_response_tree(Some(response.node), true)?.ok_or_else(|| {
            DecorationEvaluationError::RuntimeProtocol("missing composition tree".into())
        })?;
        Ok(DecorationEvaluationResult {
            node,
            transform: WindowTransform::default(),
            managed_window: ManagedWindowState::default(),
            window_effects: None,
            dirty_node_ids: Vec::new(),
            next_poll_in_ms: None,
            actions: response.actions,
        })
    }
    fn render(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let request = EvaluateRequest {
            snapshot: snapshot.clone(),
            window_id: Some(snapshot.id.clone()),
            context: Self::context(&state, now_ms),
        };
        let response = self.use_host(&mut state, |host| host.evaluate(request))?;
        let result = Self::evaluation_result(response)?;
        state
            .windows
            .insert(snapshot.id.clone(), (snapshot.clone(), result.clone()));
        Ok(result)
    }
    fn render_preview(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let request = EvaluatePreviewRequest {
            snapshot: snapshot.clone(),
            window_id: Some(snapshot.id.clone()),
            context: Self::context(&state, now_ms),
        };
        let response = self.use_host(&mut state, |host| host.evaluate_preview(request))?;
        let result = Self::evaluation_result(response)?;
        Ok(result)
    }
    fn render_candidate_preview(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let request = EvaluateCandidatePreviewRequest {
            snapshot: snapshot.clone(),
            window_id: Some(snapshot.id.clone()),
            context: Self::context(&state, now_ms),
        };
        let response =
            self.use_host(&mut state, |host| host.evaluate_candidate_preview(request))?;
        let result = Self::evaluation_result(response)?;
        Ok(result)
    }
    fn render_cached(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let request = EvaluateCachedRequest {
            snapshot: Some(snapshot.clone()),
            window_id: snapshot.id.clone(),
            context: Self::context(&state, now_ms),
        };
        let response = self.use_host(&mut state, |host| host.evaluate_cached(request))?;
        let result = Self::evaluation_result(response)?;
        state
            .windows
            .insert(snapshot.id.clone(), (snapshot.clone(), result.clone()));
        Ok(result)
    }
    fn decode_response_tree(
        wire: Option<shojiwm_lib::ssd::WireDecorationNode>,
        required: bool,
    ) -> Result<Option<shojiwm_lib::ssd::DecorationNode>, DecorationEvaluationError> {
        let Some(wire) = wire else {
            return if required {
                Err(DecorationEvaluationError::RuntimeProtocol(
                    "missing serialized composition tree".into(),
                ))
            } else {
                Ok(None)
            };
        };
        // Same conversion/structural validation as the TS wire path.
        let node: shojiwm_lib::ssd::DecorationNode = wire.try_into()?;
        DecorationTree::new(node.clone())
            .validate()
            .map_err(|error| {
                DecorationEvaluationError::RuntimeProtocol(format!(
                    "invalid composition tree: {error:?}"
                ))
            })?;
        Ok(Some(node))
    }
}

impl DecorationEvaluator for DotNetDecorationEvaluator {
    fn evaluate_window(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        self.render(snapshot, now_ms)
    }

    fn evaluate_window_preview(
        &self,
        snapshot: &WaylandWindowSnapshot,
        now_ms: u64,
    ) -> Result<DecorationEvaluationResult, DecorationEvaluationError> {
        self.render_preview(snapshot, now_ms)
    }

    fn evaluate_cached_window(
        &self,
        window_id: &str,
        snapshot: Option<&WaylandWindowSnapshot>,
        now_ms: u64,
        force_full: bool,
    ) -> Result<DecorationCachedEvaluationResult, DecorationEvaluationError> {
        if let Some(snapshot) = snapshot {
            if snapshot.id != window_id {
                return Err(DecorationEvaluationError::RuntimeProtocol(
                    "cached snapshot windowId mismatch".into(),
                ));
            }
            return self.render_cached(snapshot, now_ms).map(Into::into);
        }
        let state = self.lock()?;
        if let Some(error) = &state.failure {
            return Err(DecorationEvaluationError::RuntimeProtocol(error.clone()));
        }
        let (snapshot, result) = state.windows.get(window_id).cloned().ok_or_else(|| {
            DecorationEvaluationError::RuntimeProtocol(format!(
                "unknown cached window: {window_id}"
            ))
        })?;
        drop(state);
        if force_full {
            return self.render_cached(&snapshot, now_ms).map(Into::into);
        }
        let mut cached: DecorationCachedEvaluationResult = result.into();
        cached.node = None;
        cached.actions.clear();
        Ok(cached)
    }

    fn invoke_handler(
        &self,
        window_id: &str,
        handler_id: &str,
        now_ms: u64,
    ) -> Result<DecorationHandlerInvocation, DecorationEvaluationError> {
        let mut state = self.lock()?;
        let request = InvokeHandlerRequest {
            window_id: window_id.to_owned(),
            handler_id: handler_id.to_owned(),
            context: Self::context(&state, now_ms),
        };
        let response = self.use_host(&mut state, |host| host.invoke_handler(request))?;
        let node = Self::decode_response_tree(response.node, false)?;
        if let Some(node) = &node {
            if let Some((_, cached)) = state.windows.get_mut(window_id) {
                cached.node = node.clone();
            }
        }
        Ok(DecorationHandlerInvocation {
            invoked: response.invoked,
            node,
            actions: response.actions,
            ..Default::default()
        })
    }

    fn window_closed(&self, window_id: &str) -> Result<(), DecorationEvaluationError> {
        let mut state = self.lock()?;
        state.windows.remove(window_id);
        self.use_host(&mut state, |host| host.window_closed(window_id.to_owned()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shojiwm_lib::ssd::{
        DecorationNodeKind, WindowAction, window_model::WindowPositionSnapshot,
    };

    fn snapshot() -> WaylandWindowSnapshot {
        let rect = WindowPositionSnapshot {
            x: 0.5,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        WaylandWindowSnapshot {
            id: "1".into(),
            title: "Kitty 日本語".into(),
            app_id: Some("kitty".into()),
            position: rect,
            rect,
            is_focused: true,
            is_floating: true,
            is_maximized: false,
            is_fullscreen: false,
            is_xwayland: false,
            decoration: Default::default(),
            size_constraints: Default::default(),
            is_resizable: true,
            is_transient: false,
            parent_id: None,
            icon: None,
            interaction: Default::default(),
        }
    }

    #[test]
    fn shared_snapshot_fixture_matches_actual_rust_wire() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../dotnet/ShojiWM.Tests/Fixtures/window.json"
        ))
        .unwrap();
        assert_eq!(serde_json::to_value(snapshot()).unwrap(), fixture);
    }

    #[test]
    fn invalid_tree_is_rejected() {
        for json in [
            r#"{"kind":"Box"}"#,
            r#"{"kind":"WindowBorder","children":[]}"#,
        ] {
            let wire = serde_json::from_str(json).unwrap();
            assert!(DotNetDecorationEvaluator::decode_response_tree(Some(wire), true).is_err());
        }
    }

    fn handler_id(node: &shojiwm_lib::ssd::DecorationNode) -> Option<&str> {
        if let DecorationNodeKind::Button(button) = &node.kind
            && let WindowAction::RuntimeHandler(id) = &button.action
        {
            return Some(id);
        }
        node.children.iter().find_map(handler_id)
    }

    #[test]
    #[ignore = "build .NET projects and set SHOJI_TEST_DOTNET_RUNTIME / SHOJI_TEST_DOTNET_CONFIG"]
    fn real_dotnet_host_decodes_example_and_dispatches_delegate() {
        let evaluator = DotNetDecorationEvaluator::new(
            std::env::var_os("SHOJI_TEST_DOTNET_RUNTIME")
                .expect("runtime bootstrap DLL path")
                .into(),
            std::env::var_os("SHOJI_TEST_DOTNET_CONFIG")
                .expect("example assembly path")
                .into(),
        );
        evaluator.preload().unwrap();
        evaluator.lifecycle_enable("initial").unwrap();
        let result = evaluator.evaluate_window(&snapshot(), 1234).unwrap();
        let tree = DecorationTree::new(result.node.clone());
        tree.validate().unwrap();
        let layout = tree
            .layout(shojiwm_lib::ssd::LogicalRect::new(0, 0, 802, 630))
            .unwrap();
        assert!(layout.window_slot_rect().is_some());
        assert!(!layout.render_primitives().is_empty());
        let handler = handler_id(&result.node).expect("close handler").to_string();
        let invoked = evaluator.invoke_handler("1", &handler, 1235).unwrap();
        assert!(invoked.invoked && invoked.node.is_some());
        assert_eq!(invoked.actions.len(), 1);
        assert_eq!(invoked.actions[0].window_id, "1");
        assert_eq!(
            invoked.actions[0].action,
            shojiwm_lib::ssd::WaylandWindowAction::Close
        );
        evaluator.window_closed("1").unwrap();
        assert!(
            !evaluator
                .invoke_handler("1", &handler, 1236)
                .unwrap()
                .invoked
        );
    }
}
