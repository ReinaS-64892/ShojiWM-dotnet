use std::{path::PathBuf, sync::atomic::AtomicBool};

use shojiwm_lib::{
    runtime_api::{
        ConfigRuntime, DecorationRequest, LaunchContext, RuntimeError, RuntimeEvent,
        RuntimeLauncher, RuntimeReply, RuntimeRequest, cli::ArgSpec,
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

    fn stage(&self) -> Result<(std::sync::Arc<GenerationDirectory>, PathBuf), RuntimeError> {
        let config =
            self.context.config_path.as_ref().ok_or_else(|| {
                error("dotnet requires --config /path/to/Config.dll or SHOJI_CONFIG")
            })?;
        if let Some(project) = self.context.extra.get("dotnet-project") {
            let directory = GenerationDirectory::create().map_err(error)?;
            source::build_project(
                std::path::Path::new(project),
                directory.path(),
                &AtomicBool::new(false),
            )
            .map_err(error)?;
            let assembly = directory.path().join(
                config
                    .file_name()
                    .ok_or_else(|| error("config has no filename"))?,
            );
            if !assembly.is_file() {
                return Err(error(format!(
                    "build did not produce {} (match --config filename to AssemblyName)",
                    assembly.display()
                )));
            }
            Ok((directory, assembly))
        } else {
            GenerationDirectory::copy_config(config).map_err(error)
        }
    }
}

impl ConfigRuntime for DotNetRuntime {
    fn preload(&mut self) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        if self.evaluator.is_some() {
            return Ok(());
        }
        let (directory, assembly) = self.stage()?;
        let evaluator =
            DotNetDecorationEvaluator::for_generation(self.component(), assembly, directory)
                .with_host(self.context.host.clone());
        evaluator.preload()?;
        evaluator.set_display_state(self.displays.clone());
        evaluator.set_input_state(self.inputs.clone());
        self.evaluator = Some(evaluator);
        Ok(())
    }

    fn enable(&mut self) -> Result<(), RuntimeError> {
        if !self.enabled {
            self.evaluator()?.lifecycle_enable("initial")?;
            self.enabled = true;
        }
        self.ensure_running()
    }

    fn reload(&mut self) -> Result<(), RuntimeError> {
        self.ensure_running()?;
        // Recovery after an initial load failure is supported without restarting
        // the compositor. Normal reload always retains its permanent host thread.
        if self.evaluator.is_none() {
            self.preload()?;
            return self.enable();
        }
        let (directory, assembly) = self.stage()?;
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
