//! Permanent CoreCLR bootstrap; every foreign call and arena stays on one thread.
use super::{
    arena::{ArenaString, MAX_ARENA_SIZE, ResultArena},
    native::*,
};
use netcorehost::{hostfxr::Hostfxr, pdcstr, pdcstring::PdCString};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{OnceLock, mpsc},
    thread::{self, JoinHandle},
};
const ABI_VERSION: u32 = 6;
type Initialize = extern "system" fn(ArenaString) -> SwmStatusResult;
type Shutdown = extern "system" fn() -> SwmStatusResult;
#[derive(Clone, Copy)]
struct DotNetExports {
    initialize: Initialize,
    shutdown: Shutdown,
    arena_free: ArenaFree,
    evaluate: extern "system" fn(SwmEvaluateInput) -> SwmEvaluateResult,
    evaluate_preview: extern "system" fn(SwmEvaluatePreviewInput) -> SwmEvaluateResult,
    evaluate_candidate_preview:
        extern "system" fn(SwmEvaluateCandidatePreviewInput) -> SwmEvaluateResult,
    evaluate_cached: extern "system" fn(SwmEvaluateCachedInput) -> SwmEvaluateResult,
    invoke_handler: extern "system" fn(SwmInvokeHandlerInput) -> SwmInvokeHandlerResult,
    window_closed: extern "system" fn(ArenaString) -> SwmStatusResult,
    prepare_assembly: extern "system" fn(ArenaString) -> SwmStatusResult,
    lifecycle_enable: extern "system" fn(ArenaString) -> SwmLifecycleEnableResult,
    lifecycle_disable: extern "system" fn(ArenaString) -> SwmStatusResult,
    commit_assembly: extern "system" fn() -> SwmCommitAssemblyResult,
    abort_assembly: extern "system" fn() -> SwmStatusResult,
    drain_preload: extern "system" fn() -> SwmStatusResult,
    shutdown_assemblies: extern "system" fn() -> SwmStatusResult,
}
static BOOTSTRAP: OnceLock<Result<(PathBuf, DotNetExports), String>> = OnceLock::new();
fn component_path(path: &Path) -> Result<PathBuf, String> {
    // Accept the previous apphost spelling by resolving its sibling DLL.
    let dll = if path.extension().is_some_and(|x| x == "dll") {
        path.to_owned()
    } else {
        // "ShojiWM.Runtime" is an apphost basename, not a .Runtime extension
        // to replace. Its sibling is ShojiWM.Runtime.dll, not ShojiWM.dll.
        let mut name = path.as_os_str().to_os_string();
        name.push(".dll");
        PathBuf::from(name)
    };
    let resolved = if dll.components().count() == 1 && !dll.is_file() {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(&dll))
            .find(|p| p.is_file())
            .unwrap_or(dll)
    } else {
        dll
    };
    resolved
        .canonicalize()
        .map_err(|e| format!("bootstrap component {}: {e}", resolved.display()))
}

fn find_hostfxr() -> Result<PathBuf, String> {
    let roots = if let Some(root) = std::env::var_os("DOTNET_ROOT") {
        vec![PathBuf::from(root)]
    } else {
        let mut roots = vec![
            PathBuf::from("/usr/share/dotnet"),
            PathBuf::from("/usr/lib/dotnet"),
            PathBuf::from("/opt/dotnet"),
        ];
        if let Some(dotnet) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join("dotnet"))
            .find_map(|p| p.canonicalize().ok())
        {
            if let Some(parent) = dotnet.parent() {
                roots.insert(0, parent.to_owned());
            }
        }
        roots
    };
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root.join("host/fxr")) else {
            continue;
        };
        let mut versions: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let version = entry
                    .file_name()
                    .to_string_lossy()
                    .split('.')
                    .map(str::parse::<u32>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                let path = entry.path().join("libhostfxr.so");
                path.is_file().then_some((version, path))
            })
            .collect();
        versions.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some((_, path)) = versions.pop() {
            return Ok(path);
        }
    }
    Err(
        "hostfxr discovery failed: set DOTNET_ROOT to an installed .NET root containing host/fxr"
            .into(),
    )
}

fn bootstrap(path: &Path) -> Result<DotNetExports, String> {
    if std::mem::size_of::<usize>() != 8 {
        return Err("native ABI v6 requires 64-bit pointers".into());
    }
    let path = component_path(path)?;
    let loaded = BOOTSTRAP.get_or_init(|| {
        let runtimeconfig = path.with_extension("runtimeconfig.json");
        if !runtimeconfig.is_file() {
            return Err(format!("missing {}", runtimeconfig.display()));
        }
        // Intentionally process lifetime: retain the library even if initialization
        // fails after it has loaded. Context close is not CLR shutdown/dlclose.
        let library =
            Hostfxr::load_from_path(find_hostfxr()?).map_err(|e| format!("hostfxr load: {e}"))?;
        let library = Box::leak(Box::new(library));
        let config = PdCString::from_os_str(&runtimeconfig).map_err(|e| e.to_string())?;
        let context = library
            .initialize_for_runtime_config(config)
            .map_err(|e| format!("CoreCLR initialization: {e}"))?;
        let assembly = PdCString::from_os_str(&path).map_err(|e| e.to_string())?;
        let loader = context
            .get_delegate_loader_for_assembly(assembly)
            .map_err(|e| format!("bootstrap loader: {e}"))?;
        let name = pdcstr!("ShojiWM.Runtime.NativeEntryPoint, ShojiWM.Runtime");
        // Check before loading/calling changed signatures. A legacy bootstrap
        // without this export fails safely rather than receiving incompatible arguments.
        let version = *loader
            .get_function_with_unmanaged_callers_only::<fn() -> u32>(name, pdcstr!("GetAbiVersion"))
            .map_err(|e| format!("ABI version export: {e}"))?;
        let version = version();
        if version != ABI_VERSION {
            return Err(format!(
                "ABI version mismatch: expected {ABI_VERSION}, got {version}"
            ));
        }
        let initialize = *loader.get_function_with_unmanaged_callers_only::<fn(ArenaString) -> SwmStatusResult>(name, pdcstr!("SWMInitialize")).map_err(|e| e.to_string())?;
        let shutdown = *loader.get_function_with_unmanaged_callers_only::<fn() -> SwmStatusResult>(name, pdcstr!("SWMShutdown")).map_err(|e| e.to_string())?;
        let arena_free = *loader.get_function_with_unmanaged_callers_only::<fn(ResultArena)>(name, pdcstr!("SWMArenaFree")).map_err(|e| e.to_string())?;
        let evaluate = *loader.get_function_with_unmanaged_callers_only::<fn(SwmEvaluateInput) -> SwmEvaluateResult>(name, pdcstr!("SWMEvaluate")).map_err(|e| e.to_string())?;
        let evaluate_preview = *loader.get_function_with_unmanaged_callers_only::<fn(SwmEvaluatePreviewInput) -> SwmEvaluateResult>(name, pdcstr!("SWMEvaluatePreview")).map_err(|e| e.to_string())?;
        let evaluate_candidate_preview = *loader.get_function_with_unmanaged_callers_only::<fn(SwmEvaluateCandidatePreviewInput) -> SwmEvaluateResult>(name, pdcstr!("SWMEvaluateCandidatePreview")).map_err(|e| e.to_string())?;
        let evaluate_cached = *loader.get_function_with_unmanaged_callers_only::<fn(SwmEvaluateCachedInput) -> SwmEvaluateResult>(name, pdcstr!("SWMEvaluateCached")).map_err(|e| e.to_string())?;
        let invoke_handler = *loader.get_function_with_unmanaged_callers_only::<fn(SwmInvokeHandlerInput) -> SwmInvokeHandlerResult>(name, pdcstr!("SWMInvokeHandler")).map_err(|e| e.to_string())?;
        let window_closed = *loader.get_function_with_unmanaged_callers_only::<fn(ArenaString) -> SwmStatusResult>(name, pdcstr!("SWMWindowClosed")).map_err(|e| e.to_string())?;
        let prepare_assembly = *loader.get_function_with_unmanaged_callers_only::<fn(ArenaString) -> SwmStatusResult>(name, pdcstr!("SWMPrepareAssembly")).map_err(|e| e.to_string())?;
        let lifecycle_enable = *loader.get_function_with_unmanaged_callers_only::<fn(ArenaString) -> SwmLifecycleEnableResult>(name, pdcstr!("SWMLifecycleEnable")).map_err(|e| e.to_string())?;
        let lifecycle_disable = *loader.get_function_with_unmanaged_callers_only::<fn(ArenaString) -> SwmStatusResult>(name, pdcstr!("SWMLifecycleDisable")).map_err(|e| e.to_string())?;
        let commit_assembly = *loader.get_function_with_unmanaged_callers_only::<fn() -> SwmCommitAssemblyResult>(name, pdcstr!("SWMCommitAssembly")).map_err(|e| e.to_string())?;
        let abort_assembly = *loader.get_function_with_unmanaged_callers_only::<fn() -> SwmStatusResult>(name, pdcstr!("SWMAbortAssembly")).map_err(|e| e.to_string())?;
        let drain_preload = *loader.get_function_with_unmanaged_callers_only::<fn() -> SwmStatusResult>(name, pdcstr!("SWMDrainPreload")).map_err(|e| e.to_string())?;
        let shutdown_assemblies = *loader.get_function_with_unmanaged_callers_only::<fn() -> SwmStatusResult>(name, pdcstr!("SWMShutdownAssemblies")).map_err(|e| e.to_string())?;
        Ok((path.clone(), DotNetExports { initialize, shutdown, arena_free, evaluate, evaluate_preview, evaluate_candidate_preview, evaluate_cached, invoke_handler, window_closed, prepare_assembly, lifecycle_enable, lifecycle_disable, commit_assembly, abort_assembly, drain_preload, shutdown_assemblies }))
    });
    match loaded {
        Ok((active, api)) if active == &path => Ok(*api),
        Ok(_) => Err("bootstrap path changed; ShojiWM restart required".into()),
        Err(error) => Err(format!("{error}; ShojiWM restart required")),
    }
}

struct ManagedHost {
    api: DotNetExports,
    owner_thread: PhantomData<Rc<()>>,
}
impl ManagedHost {
    fn initialize(api: DotNetExports, path: &Path) -> Result<Self, String> {
        let bytes = path.to_str().ok_or("config path must be UTF-8")?.as_bytes();
        if bytes.len() > MAX_ARENA_SIZE {
            return Err("config path too long".into());
        }
        let result = (api.initialize)(ArenaString {
            ptr: bytes.as_ptr(),
            length: bytes.len() as i32,
        });
        let host = (result.status == 0).then(|| Self {
            api,
            owner_thread: PhantomData,
        });
        result.decode(api.arena_free).map_err(|e| e.to_string())?;
        host.ok_or_else(|| "managed initialization did not establish ownership".into())
    }
    fn evaluate(&self, request: EvaluateRequest) -> Result<EvaluationResponse, BridgeError> {
        let (_input_owner, input) = request.borrowed()?;
        (self.api.evaluate)(input).decode(self.api.arena_free)
    }
    fn evaluate_preview(
        &self,
        request: EvaluatePreviewRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        let (_input_owner, input) = request.borrowed()?;
        (self.api.evaluate_preview)(input).decode(self.api.arena_free)
    }
    fn evaluate_candidate_preview(
        &self,
        request: EvaluateCandidatePreviewRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        let (_input_owner, input) = request.borrowed()?;
        (self.api.evaluate_candidate_preview)(input).decode(self.api.arena_free)
    }
    fn evaluate_cached(
        &self,
        request: EvaluateCachedRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        let (_input_owner, input) = request.borrowed()?;
        (self.api.evaluate_cached)(input).decode(self.api.arena_free)
    }
    fn invoke_handler(
        &self,
        request: InvokeHandlerRequest,
    ) -> Result<HandlerResponse, BridgeError> {
        let (_input_owner, input) = request.borrowed()?;
        (self.api.invoke_handler)(input).decode(self.api.arena_free)
    }
    fn window_closed(&self, request: String) -> Result<(), BridgeError> {
        if request.len() > MAX_ARENA_SIZE {
            return Err(BridgeError::Host("input string exceeds 8 MiB limit".into()));
        }
        let input = ArenaString {
            ptr: request.as_ptr(),
            length: request.len() as i32,
        };
        (self.api.window_closed)(input).decode(self.api.arena_free)
    }
    fn prepare_assembly(&self, request: String) -> Result<(), BridgeError> {
        if request.len() > MAX_ARENA_SIZE {
            return Err(BridgeError::Host("input string exceeds 8 MiB limit".into()));
        }
        let input = ArenaString {
            ptr: request.as_ptr(),
            length: request.len() as i32,
        };
        (self.api.prepare_assembly)(input).decode(self.api.arena_free)
    }
    fn lifecycle_enable(&self, request: String) -> Result<EnableResponse, BridgeError> {
        if request.len() > MAX_ARENA_SIZE {
            return Err(BridgeError::Host("input string exceeds 8 MiB limit".into()));
        }
        let input = ArenaString {
            ptr: request.as_ptr(),
            length: request.len() as i32,
        };
        (self.api.lifecycle_enable)(input).decode(self.api.arena_free)
    }
    fn lifecycle_disable(&self, request: String) -> Result<(), BridgeError> {
        if request.len() > MAX_ARENA_SIZE {
            return Err(BridgeError::Host("input string exceeds 8 MiB limit".into()));
        }
        let input = ArenaString {
            ptr: request.as_ptr(),
            length: request.len() as i32,
        };
        (self.api.lifecycle_disable)(input).decode(self.api.arena_free)
    }
    fn commit_assembly(&self) -> Result<CommitResponse, BridgeError> {
        (self.api.commit_assembly)().decode(self.api.arena_free)
    }
    fn abort_assembly(&self) -> Result<(), BridgeError> {
        (self.api.abort_assembly)().decode(self.api.arena_free)
    }
    fn drain_preload(&self) -> Result<(), BridgeError> {
        (self.api.drain_preload)().decode(self.api.arena_free)
    }
    fn shutdown_assemblies(&self) -> Result<(), BridgeError> {
        (self.api.shutdown_assemblies)().decode(self.api.arena_free)
    }
}
impl Drop for ManagedHost {
    fn drop(&mut self) {
        if let Err(error) = (self.api.shutdown)().decode(self.api.arena_free) {
            eprintln!("ShojiWM .NET shutdown: {error}");
        }
    }
}
// Jobs carry only Rust-owned inputs/results. Their function identity is preserved;
// there is no operation tag or enum dispatcher on the managed thread.
type RuntimeJob = Box<dyn FnOnce(&ManagedHost) + Send>;
#[derive(Debug)]
pub struct InProcessDotNetHost {
    commands: Option<mpsc::Sender<RuntimeJob>>,
    thread: Option<JoinHandle<()>>,
}
impl InProcessDotNetHost {
    pub fn start(component: &Path, config: &Path) -> Result<Self, String> {
        let component = component.to_owned();
        let config = config.to_owned();
        let (commands, receiver) = mpsc::channel::<RuntimeJob>();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("dotnet-runtime".into())
            .spawn(move || {
                let host = match bootstrap(&component)
                    .and_then(|api| ManagedHost::initialize(api, &config))
                {
                    Ok(host) => host,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(job) = receiver.recv() {
                    job(&host);
                }
            })
            .map_err(|e| e.to_string())?;
        if let Err(e) = ready_rx.recv().map_err(|e| e.to_string()).and_then(|r| r) {
            let _ = thread.join();
            return Err(e);
        }
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
        })
    }
    fn on_runtime_thread<T: Send + 'static>(
        &mut self,
        job: impl FnOnce(&ManagedHost) -> Result<T, BridgeError> + Send + 'static,
    ) -> Result<T, BridgeError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or_else(|| {
                BridgeError::Host("runtime host unavailable; ShojiWM restart required".into())
            })?
            .send(Box::new(move |host| {
                let _ = tx.send(job(host));
            }))
            .map_err(|e| BridgeError::Host(e.to_string()))?;
        rx.recv().map_err(|e| BridgeError::Host(e.to_string()))?
    }
    pub fn evaluate(
        &mut self,
        request: EvaluateRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.evaluate(request))
    }
    pub fn evaluate_preview(
        &mut self,
        request: EvaluatePreviewRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.evaluate_preview(request))
    }
    pub fn evaluate_candidate_preview(
        &mut self,
        request: EvaluateCandidatePreviewRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.evaluate_candidate_preview(request))
    }
    pub fn evaluate_cached(
        &mut self,
        request: EvaluateCachedRequest,
    ) -> Result<EvaluationResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.evaluate_cached(request))
    }
    pub fn invoke_handler(
        &mut self,
        request: InvokeHandlerRequest,
    ) -> Result<HandlerResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.invoke_handler(request))
    }
    pub fn window_closed(&mut self, request: String) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.window_closed(request))
    }
    pub fn prepare_assembly(&mut self, request: String) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.prepare_assembly(request))
    }
    pub fn lifecycle_enable(&mut self, request: String) -> Result<EnableResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.lifecycle_enable(request))
    }
    pub fn lifecycle_disable(&mut self, request: String) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.lifecycle_disable(request))
    }
    pub fn commit_assembly(&mut self) -> Result<CommitResponse, BridgeError> {
        self.on_runtime_thread(move |host| host.commit_assembly())
    }
    pub fn abort_assembly(&mut self) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.abort_assembly())
    }
    pub fn drain_preload(&mut self) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.drain_preload())
    }
    pub fn shutdown_assemblies(&mut self) -> Result<(), BridgeError> {
        self.on_runtime_thread(move |host| host.shutdown_assemblies())
    }
    pub fn stop(&mut self) {
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for InProcessDotNetHost {
    fn drop(&mut self) {
        self.stop();
    }
}
