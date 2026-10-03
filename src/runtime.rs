use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use shojiwm_lib::{
    runtime_api::{
        ConfigRuntime, DecorationRequest, HostMessage, LaunchContext, ReloadPreparation,
        RuntimeError, RuntimeEvent, RuntimeHost, RuntimeLauncher, RuntimeReply, RuntimeRequest,
        cli::ArgSpec,
    },
    ssd::DecorationEvaluator,
};

use crate::bridge::{assembly::GenerationDirectory, evaluator::DotNetDecorationEvaluator, source};

#[derive(Debug, Clone, Copy, Default)]
pub struct DotNetLauncher;

const EXTRA_ARGS: &[ArgSpec] = &[
    ArgSpec {
        name: "decoration-runtime",
        env: Some("SHOJI_DECORATION_RUNTIME"),
        value_name: "DLL",
        help: "Permanent .NET bootstrap assembly",
    },
    ArgSpec {
        name: "dotnet-project",
        env: Some("SHOJI_DOTNET_PROJECT"),
        value_name: "CSPROJ",
        help: "Build this configuration project on preload and reload",
    },
];

impl RuntimeLauncher for DotNetLauncher {
    fn name(&self) -> &'static str {
        "dotnet"
    }

    fn extra_args(&self) -> &'static [ArgSpec] {
        EXTRA_ARGS
    }

    fn launch(&self, context: LaunchContext) -> Box<dyn ConfigRuntime> {
        // Construction is inert: no filesystem reads, CLR startup or config code.
        Box::new(DotNetRuntime::new(context))
    }
}

/// Owns the managed runtime through the public lifecycle. The bridge serializes
/// borrowed requests before sending owned bytes to the dedicated managed thread.
pub struct DotNetRuntime {
    context: LaunchContext,
    evaluator: Option<DotNetDecorationEvaluator>,
    preparation: Option<ReloadJob>,
    enabled: bool,
    stopped: bool,
    displays: std::collections::BTreeMap<String, shojiwm_lib::ssd::WaylandOutputSnapshot>,
    inputs:
        std::collections::BTreeMap<String, shojiwm_lib::runtime_input::RuntimeInputDeviceSnapshot>,
}

fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::RuntimeProtocol(message.into())
}

impl DotNetRuntime {
    pub fn new(context: LaunchContext) -> Self {
        Self {
            context,
            evaluator: None,
            preparation: None,
            enabled: false,
            stopped: false,
            displays: Default::default(),
            inputs: Default::default(),
        }
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        if self.stopped {
            Err(error(".NET runtime has been shut down"))
        } else {
            Ok(())
        }
    }

    fn evaluator(&self) -> Result<&DotNetDecorationEvaluator, RuntimeError> {
        self.ensure_running()?;
        self.evaluator
            .as_ref()
            .ok_or_else(|| error(".NET config has not been preloaded"))
    }

    fn component(&self) -> PathBuf {
        if let Some(path) = self.context.extra.get("decoration-runtime") {
            return PathBuf::from(path);
        }
        if let Some(dir) = &self.context.runtime_dir {
            return dir.join("ShojiWM.Runtime.dll");
        }
        if self.context.dev {
            return PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("dotnet/ShojiWM.Runtime/bin/Release/net10.0/ShojiWM.Runtime.dll");
        }
        PathBuf::from("ShojiWM.Runtime.dll")
    }

    fn stage(&self) -> Result<StagedConfig, RuntimeError> {
        stage_config(
            self.config_path()?,
            self.project().as_deref(),
            &AtomicBool::new(false),
        )
        .map_err(error)
    }

    fn config_path(&self) -> Result<&Path, RuntimeError> {
        self.context
            .config_path
            .as_deref()
            .ok_or_else(|| error("dotnet requires --config /path/to/Config.dll or SHOJI_CONFIG"))
    }

    fn project(&self) -> Option<PathBuf> {
        self.context.extra.get("dotnet-project").map(PathBuf::from)
    }

    fn load_initial(&mut self, (directory, assembly): StagedConfig) -> Result<(), RuntimeError> {
        let evaluator =
            DotNetDecorationEvaluator::for_generation(self.component(), assembly, directory)
                .with_host(self.context.host.clone());
        evaluator.preload()?;
        evaluator.set_display_state(self.displays.clone());
        evaluator.set_input_state(self.inputs.clone());
        self.evaluator = Some(evaluator);
        Ok(())
    }
}

type StagedConfig = (Arc<GenerationDirectory>, PathBuf);

fn stage_config(
    config: &Path,
    project: Option<&Path>,
    stop: &AtomicBool,
) -> Result<StagedConfig, String> {
    if let Some(project) = project {
        let directory = GenerationDirectory::create()?;
        source::build_project(project, directory.path(), stop)?;
        let assembly = directory
            .path()
            .join(config.file_name().ok_or("config has no filename")?);
        if !assembly.is_file() {
            return Err(format!(
                "build did not produce {} (match --config filename to AssemblyName)",
                assembly.display()
            ));
        }
        Ok((directory, assembly))
    } else {
        GenerationDirectory::copy_config(config)
    }
}

/// Only owned paths/output cross this worker boundary. It never holds the
/// evaluator lock or calls managed code, so active requests remain usable.
struct ReloadJob {
    stop: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<StagedConfig, String>>>>,
    worker: Option<JoinHandle<()>>,
}

impl ReloadJob {
    fn spawn(
        host: RuntimeHost,
        prepare: impl FnOnce(&AtomicBool) -> Result<StagedConfig, String> + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let worker_stop = stop.clone();
        let worker_result = result.clone();
        let worker = thread::Builder::new()
            .name("dotnet-reload".into())
            .spawn(move || {
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare(&worker_stop)
                }))
                .unwrap_or_else(|_| Err(".NET reload worker panicked".into()));
                let notification = prepared.as_ref().map(|_| ()).map_err(Clone::clone);
                // Store before notifying, under the same lock, so a retry cannot
                // overtake an old failure notification.
                let mut result = worker_result.lock().unwrap();
                if !worker_stop.load(Ordering::Acquire) {
                    *result = Some(prepared);
                    host.send(HostMessage::ReloadReady(notification));
                }
            })
            .map_err(|e| format!("could not start reload worker: {e}"))?;
        Ok(Self {
            stop,
            result,
            worker: Some(worker),
        })
    }
}

impl Drop for ReloadJob {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // Result drops here, releasing unpublished staging directories.
    }
}

impl ConfigRuntime for DotNetRuntime {
    fn preload(&mut self) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        if self.evaluator.is_some() {
            return Ok(());
        }
        self.load_initial(self.stage()?)
    }

    fn enable(&mut self) -> Result<(), RuntimeError> {
        if !self.enabled {
            self.evaluator()?.lifecycle_enable("initial")?;
            self.enabled = true;
        }
        self.ensure_running()
    }

    fn prepare_reload(&mut self) -> Result<ReloadPreparation, RuntimeError> {
        self.ensure_running()?;
        if let Some(job) = &self.preparation {
            // Coalesce repeated reload keys. A failed build has already notified
            // upstream (which does not call reload on failure), so allow retry.
            if !job
                .result
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(Result::is_err)
            {
                return Ok(ReloadPreparation::Pending);
            }
        }
        self.preparation.take();
        let config = self.config_path()?.to_owned();
        let project = self.project();
        self.preparation = Some(
            ReloadJob::spawn(self.context.host.clone(), move |stop| {
                stage_config(&config, project.as_deref(), stop)
            })
            .map_err(error)?,
        );
        Ok(ReloadPreparation::Pending)
    }

    fn reload(&mut self) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        let job = self
            .preparation
            .as_ref()
            .ok_or_else(|| error("reload requires prepare_reload first"))?;
        let prepared = job
            .result
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| error(".NET reload preparation is still running"))?;
        self.preparation.take();
        let (directory, assembly) = prepared.map_err(error)?;
        // Recovery uses the prepared output too; never compile again at commit.
        if self.evaluator.is_none() {
            self.load_initial((directory, assembly))?;
            return self.enable();
        }
        let current = self.evaluator()?;
        if !current.has_active_host() {
            return Err(error(".NET host is unavailable; restart required"));
        }
        let next = current.prepare_assembly(assembly, directory)?;
        for snapshot in current.window_snapshots()? {
            // Same public wire conversion and structural validation as startup.
            // Dropping next on error aborts candidate and retains active handlers.
            next.validate_candidate(&snapshot)?;
        }
        next.activate_prepared()?;
        self.evaluator = Some(next);
        self.enabled = true;
        // The compositor invalidates its caches after this returns success.
        Ok(())
    }

    fn request(
        &mut self,
        now_ms: f64,
        request: RuntimeRequest<'_>,
    ) -> Result<RuntimeReply, RuntimeError> {
        self.ensure_running()?;
        let now = now_ms as u64;
        Ok(match request {
            RuntimeRequest::Decoration(DecorationRequest::Evaluate { window, preview }) => {
                let evaluator = self.evaluator()?;
                let result = if preview {
                    evaluator.evaluate_window_preview(window, now)?
                } else {
                    evaluator.evaluate_window(window, now)?
                };
                RuntimeReply::Evaluation(Box::new(result))
            }
            RuntimeRequest::Decoration(DecorationRequest::EvaluateCached {
                window_id,
                window,
                force_full,
            }) => RuntimeReply::CachedEvaluation(Box::new(
                self.evaluator()?
                    .evaluate_cached_window(window_id, window, now, force_full)?,
            )),
            RuntimeRequest::Decoration(DecorationRequest::InvokeHandler {
                window_id,
                handler_id,
            }) => RuntimeReply::Handler(Box::new(
                self.evaluator()?
                    .invoke_handler(window_id, handler_id, now)?,
            )),
            RuntimeRequest::Decoration(DecorationRequest::Closed { window_id }) => {
                self.evaluator()?.window_closed(window_id)?;
                RuntimeReply::Done
            }
            // Old .NET implementation used DecorationEvaluator's defaults here.
            // Generic upstream fallback preserves those semantics.
            _ => RuntimeReply::Unhandled,
        })
    }

    fn post(&mut self, _now_ms: f64, event: RuntimeEvent) {
        if self.stopped {
            return;
        }
        // Owned notifications are fire-and-forget; no managed reply is awaited.
        // Environment snapshots are copied into the next semantic request.
        match event {
            RuntimeEvent::DisplayState(state) => {
                self.displays = state;
                if let Some(evaluator) = &self.evaluator {
                    evaluator.set_display_state(self.displays.clone());
                }
            }
            RuntimeEvent::InputState(state) => {
                self.inputs = state;
                if let Some(evaluator) = &self.evaluator {
                    evaluator.set_input_state(self.inputs.clone());
                }
            }
            // No keyboard-layout or async pointer/gesture API in old .NET.
            _ => {}
        }
    }

    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.preparation.take();
        if let Some(evaluator) = self.evaluator.take() {
            evaluator.retire("shutdown");
        }
        self.enabled = false;
        self.displays.clear();
        self.inputs.clear();
    }
}

impl Drop for DotNetRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shojiwm_lib::runtime_api::{EffectRequest, RuntimeHost, cli::CommonArgs};

    fn context() -> LaunchContext {
        LaunchContext {
            config_path: None,
            runtime_dir: None,
            dev: false,
            extra: Default::default(),
            host: RuntimeHost::detached(),
        }
    }

    #[test]
    fn preparation_is_nonblocking_coalesced_and_commits_only_when_ready() {
        let host = RuntimeHost::detached();
        let mut ctx = context();
        ctx.host = host.clone();
        let mut runtime = DotNetRuntime::new(ctx);
        let (release, wait) = std::sync::mpsc::channel();
        runtime.preparation = Some(
            ReloadJob::spawn(host.clone(), move |_| {
                wait.recv().unwrap();
                Err("test failure".into())
            })
            .unwrap(),
        );
        assert!(matches!(
            runtime.prepare_reload().unwrap(),
            ReloadPreparation::Pending
        ));
        assert!(
            runtime
                .reload()
                .unwrap_err()
                .to_string()
                .contains("still running")
        );
        assert!(matches!(
            runtime.request(0.0, RuntimeRequest::SchedulerTick).unwrap(),
            RuntimeReply::Unhandled
        ));
        assert!(host.pop().is_none());
        release.send(()).unwrap();
        runtime
            .preparation
            .as_mut()
            .unwrap()
            .worker
            .take()
            .unwrap()
            .join()
            .unwrap();
        assert!(
            matches!(host.pop(), Some(HostMessage::ReloadReady(Err(message))) if message == "test failure")
        );
        assert!(
            runtime
                .reload()
                .unwrap_err()
                .to_string()
                .contains("test failure")
        );
        assert!(runtime.preparation.is_none());
        assert!(host.pop().is_none());
    }

    #[test]
    fn shutdown_cancels_and_joins_preparation_without_publishing() {
        let host = RuntimeHost::detached();
        let mut runtime = DotNetRuntime::new(context());
        let (started, wait) = std::sync::mpsc::channel();
        runtime.preparation = Some(
            ReloadJob::spawn(host.clone(), move |stop| {
                let directory = GenerationDirectory::create()?;
                started.send(directory.path().to_owned()).unwrap();
                while !stop.load(Ordering::Acquire) {
                    thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok((directory, "unused.dll".into()))
            })
            .unwrap(),
        );
        let path = wait.recv().unwrap();
        runtime.shutdown();
        assert!(!path.exists());
        assert!(host.pop().is_none());
        assert!(runtime.prepare_reload().is_err());
    }

    #[test]
    fn launcher_is_inert_and_uses_public_cli() {
        let launcher = DotNetLauncher;
        assert_eq!(launcher.name(), "dotnet");
        assert_eq!(launcher.default_config_path(false), None);
        let args = CommonArgs::parse(
            &[
                "--dotnet-project=Config.csproj".into(),
                "--decoration-runtime=Host.dll".into(),
                "--config=Config.dll".into(),
            ],
            launcher.extra_args(),
        );
        let mut ctx = context();
        ctx.extra = args.extra;
        ctx.config_path = args.config_path;
        ctx.runtime_dir = Some("/runtime".into());
        ctx.dev = true;
        let runtime = DotNetRuntime::new(ctx);
        assert_eq!(runtime.component(), PathBuf::from("Host.dll"));
        assert_eq!(runtime.context.config_path, Some("Config.dll".into()));
        assert_eq!(runtime.context.extra["dotnet-project"], "Config.csproj");
        assert!(runtime.context.dev);
        assert!(runtime.evaluator.is_none());
    }

    #[test]
    fn missing_config_errors_fallback_and_shutdown_are_safe() {
        let mut runtime = DotNetRuntime::new(context());
        assert!(runtime.enable().is_err());
        assert!(runtime.preload().is_err());
        assert!(matches!(
            runtime
                .request(0.0, RuntimeRequest::Effect(EffectRequest::Background))
                .unwrap(),
            RuntimeReply::Unhandled
        ));
        assert!(matches!(
            runtime.request(0.0, RuntimeRequest::SchedulerTick).unwrap(),
            RuntimeReply::Unhandled
        ));
        runtime.shutdown();
        runtime.shutdown();
        assert!(runtime.preload().is_err());
        assert!(runtime.reload().is_err());
        assert!(runtime.request(0.0, RuntimeRequest::SchedulerTick).is_err());
    }

    #[test]
    fn runtime_dir_and_dev_paths_are_resolved_from_context() {
        let mut ctx = context();
        ctx.runtime_dir = Some("/runtime".into());
        let runtime = DotNetRuntime::new(ctx);
        assert_eq!(
            runtime.component(),
            PathBuf::from("/runtime/ShojiWM.Runtime.dll")
        );
        let mut ctx = context();
        ctx.dev = true;
        assert!(
            DotNetRuntime::new(ctx)
                .component()
                .ends_with("dotnet/ShojiWM.Runtime/bin/Release/net10.0/ShojiWM.Runtime.dll")
        );
    }
}
