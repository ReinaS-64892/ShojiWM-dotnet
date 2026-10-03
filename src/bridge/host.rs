//! In-process CoreCLR hosting. Only bootstrap function pointers cross this
//! boundary. Config objects stay managed; all calls for a host run on one thread.
use netcorehost::{hostfxr::Hostfxr, pdcstr, pdcstring::PdCString};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{OnceLock, mpsc},
    thread::{self, JoinHandle},
};

pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const ABI_VERSION: u32 = 3;

/// Immutable UTF-8 borrowed for one synchronous call. Only pass this view while
/// its source slice is live; managed code must copy before retaining the text.
#[repr(C)]
struct RustString {
    ptr: *const u8,
    length: i32,
}
impl RustString {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("request exceeds 8 MiB message limit".into());
        }
        Ok(Self {
            ptr: bytes.as_ptr(),
            length: bytes.len() as i32,
        })
    }
}

/// Immutable UTF-8 whose ownership is transferred from .NET to Rust. Only the
/// permanent bootstrap's FreeDotnetString may release it, exactly once.
/// This raw ABI value is deliberately not Clone/Copy; use OwnedDotnetString.
#[repr(C)]
struct DotnetString {
    ptr: *const u8,
    length: i32,
}
impl DotnetString {
    fn empty() -> Self {
        Self {
            ptr: std::ptr::null(),
            length: 0,
        }
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeStatus {
    Success = 0,
    Failure = 1,
    AllocationFailure = 2,
}

/// Status plus an owned response, returned by value. Keep the status as raw i32
/// at the foreign boundary so an unknown discriminant is an error, never Rust UB.
#[repr(C)]
struct NativeResult {
    status: i32,
    response: DotnetString,
}
impl NativeResult {
    fn into_bytes(self, api: &Bootstrap, operation: &str) -> Result<Vec<u8>, String> {
        let response = OwnedDotnetString {
            value: self.response,
            api,
        }
        .into_bytes()?;
        match self.status {
            status if status == NativeStatus::Success as i32 => Ok(response),
            status => {
                let status = match status {
                    status if status == NativeStatus::Failure as i32 => "failure".to_owned(),
                    status if status == NativeStatus::AllocationFailure as i32 => {
                        "allocation failure".to_owned()
                    }
                    status => format!("unknown status {status}"),
                };
                Err(format!(
                    "managed {operation} ({status}): {}",
                    String::from_utf8_lossy(&response)
                ))
            }
        }
    }
}

type Initialize = extern "system" fn(RustString) -> NativeResult;
type Invoke = extern "system" fn(RustString) -> NativeResult;
type Shutdown = extern "system" fn() -> NativeResult;
type Free = extern "system" fn(DotnetString);

#[derive(Clone, Copy)]
struct Bootstrap {
    initialize: Initialize,
    invoke: Invoke,
    shutdown: Shutdown,
    free: Free,
}
static BOOTSTRAP: OnceLock<Result<(PathBuf, Bootstrap), String>> = OnceLock::new();

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

fn bootstrap(path: &Path) -> Result<Bootstrap, String> {
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
        // Signatures exactly match the permanent bootstrap's blittable ABI.
        let initialize = *loader
            .get_function_with_unmanaged_callers_only::<fn(RustString) -> NativeResult>(
                name,
                pdcstr!("Initialize"),
            )
            .map_err(|e| e.to_string())?;
        let invoke = *loader
            .get_function_with_unmanaged_callers_only::<fn(RustString) -> NativeResult>(
                name,
                pdcstr!("Invoke"),
            )
            .map_err(|e| e.to_string())?;
        let shutdown = *loader
            .get_function_with_unmanaged_callers_only::<fn() -> NativeResult>(
                name,
                pdcstr!("Shutdown"),
            )
            .map_err(|e| e.to_string())?;
        let free = *loader
            .get_function_with_unmanaged_callers_only::<fn(DotnetString)>(
                name,
                pdcstr!("FreeDotnetString"),
            )
            .map_err(|e| e.to_string())?;
        Ok((
            path.clone(),
            Bootstrap {
                initialize,
                invoke,
                shutdown,
                free,
            },
        ))
    });
    match loaded {
        Ok((active, api)) if active == &path => Ok(*api),
        Ok(_) => Err("bootstrap path changed; ShojiWM restart required".into()),
        Err(error) => Err(format!("{error}; ShojiWM restart required")),
    }
}

struct OwnedDotnetString<'a> {
    value: DotnetString,
    api: &'a Bootstrap,
}
impl<'a> OwnedDotnetString<'a> {
    #[cfg(test)]
    fn new(api: &'a Bootstrap) -> Self {
        Self {
            value: DotnetString::empty(),
            api,
        }
    }

    // Consume the owner: the foreign allocation is freed immediately after the
    // copy, including all validation/UTF-8 failure paths. Only Rust-owned bytes escape.
    fn into_bytes(self) -> Result<Vec<u8>, String> {
        if self.value.length < 0
            || self.value.length as usize > MAX_MESSAGE_BYTES
            || (self.value.ptr.is_null() && self.value.length != 0)
        {
            return Err("invalid native response buffer or message exceeds 8 MiB limit".into());
        }
        if self.value.length == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: bootstrap owns a readable allocation of length bytes until
        // FreeDotnetString. We copy before the RAII guard releases it.
        let bytes =
            unsafe { std::slice::from_raw_parts(self.value.ptr, self.value.length as usize) };
        std::str::from_utf8(bytes).map_err(|e| format!("invalid DotnetString UTF-8: {e}"))?;
        Ok(bytes.to_vec())
    }
}
impl Drop for OwnedDotnetString<'_> {
    fn drop(&mut self) {
        (self.api.free)(std::mem::replace(&mut self.value, DotnetString::empty()));
    }
}

struct ManagedHost {
    api: Bootstrap,
    // A unique lifetime token, confined to the initializing thread. It is neither
    // Clone nor Send/Sync; dropping it shuts down the single static managed host.
    owner_thread: PhantomData<Rc<()>>,
}
impl ManagedHost {
    fn initialize(api: Bootstrap, path: &Path) -> Result<Self, String> {
        let bytes = path.to_str().ok_or("config path must be UTF-8")?.as_bytes();
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("config path too long".into());
        }
        let result = (api.initialize)(RustString::new(bytes)?);
        // Own successful initialization before validating its response, so an
        // early error drops the token and releases the static managed host.
        let host = (result.status == NativeStatus::Success as i32).then(|| Self {
            api,
            owner_thread: PhantomData,
        });
        result.into_bytes(&api, "initialization")?;
        host.ok_or_else(|| "managed initialization did not establish ownership".into())
    }
    fn exchange(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("request exceeds 8 MiB message limit".into());
        }
        (self.api.invoke)(RustString::new(bytes)?).into_bytes(&self.api, "invocation")
    }
}
impl Drop for ManagedHost {
    fn drop(&mut self) {
        if let Err(error) = (self.api.shutdown)().into_bytes(&self.api, "shutdown") {
            eprintln!("ShojiWM .NET {error}");
        }
    }
}

enum Command {
    Invoke(Vec<u8>, mpsc::SyncSender<Result<Vec<u8>, String>>),
}
#[derive(Debug)]
pub struct InProcessDotNetHost {
    commands: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}
impl InProcessDotNetHost {
    pub fn start(component: &Path, config: &Path) -> Result<Self, String> {
        let component = component.to_owned();
        let config = config.to_owned();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("dotnet-runtime".into())
            .spawn(move || {
                let result =
                    bootstrap(&component).and_then(|api| ManagedHost::initialize(api, &config));
                let host = match result {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(Command::Invoke(bytes, reply)) = receiver.recv() {
                    let _ = reply.send(host.exchange(&bytes));
                }
                // Host destruction on its original thread, then thread joins. No
                // timeout, abandoned execution, CLR restart or child process.
            })
            .map_err(|e| e.to_string())?;
        let ready = ready_rx.recv().map_err(|e| e.to_string()).and_then(|r| r);
        if let Err(error) = ready {
            let _ = thread.join();
            return Err(error);
        }
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
        })
    }
    pub fn exchange(&mut self, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("request exceeds 8 MiB message limit".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or("runtime host unavailable; ShojiWM restart required")?
            .send(Command::Invoke(bytes, tx))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_abi_layout_and_borrowed_bytes() {
        let size = if std::mem::size_of::<usize>() == 8 {
            16
        } else {
            8
        };
        assert_eq!(std::mem::size_of::<RustString>(), size);
        assert_eq!(std::mem::size_of::<DotnetString>(), size);
        assert_eq!(std::mem::size_of::<NativeStatus>(), 4);
        assert_eq!(
            std::mem::size_of::<NativeResult>(),
            if size == 16 { 24 } else { 12 }
        );
        assert_eq!(std::mem::offset_of!(NativeResult, status), 0);
        assert_eq!(
            std::mem::offset_of!(NativeResult, response),
            std::mem::size_of::<usize>()
        );
        assert_eq!(std::mem::offset_of!(RustString, ptr), 0);
        assert_eq!(std::mem::offset_of!(DotnetString, ptr), 0);
        assert_eq!(
            std::mem::offset_of!(RustString, length),
            std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::offset_of!(DotnetString, length),
            std::mem::size_of::<usize>()
        );
        let text = "日本語🙂\0末尾";
        let borrowed = RustString::new(text.as_bytes()).unwrap();
        assert_eq!(borrowed.ptr, text.as_ptr());
        assert_eq!(borrowed.length as usize, text.len());
        assert_eq!(RustString::new(&[]).unwrap().length, 0);
        assert!(RustString::new(&vec![0; MAX_MESSAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn owned_dotnet_string_frees_once_on_copy_validation_error_and_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static FREED: AtomicUsize = AtomicUsize::new(0);
        extern "system" fn free(value: DotnetString) {
            FREED.fetch_add(1, Ordering::SeqCst);
            if !value.ptr.is_null() {
                // Test allocations are one Box<u8>, independently of the ABI length.
                unsafe {
                    drop(Box::from_raw(value.ptr as *mut u8));
                }
            }
        }
        extern "system" fn initialize(_: RustString) -> NativeResult {
            NativeResult {
                status: NativeStatus::Success as i32,
                response: DotnetString::empty(),
            }
        }
        extern "system" fn invoke(_: RustString) -> NativeResult {
            initialize(RustString::new(&[]).unwrap())
        }
        extern "system" fn shutdown() -> NativeResult {
            initialize(RustString::new(&[]).unwrap())
        }
        let api = Bootstrap {
            initialize,
            invoke,
            shutdown,
            free,
        };
        let cases = [
            (Some(b'x'), 1, true),
            (None, 0, true),
            (Some(b'x'), 0, true),
            (None, 1, false),
            (Some(b'x'), -1, false),
            (Some(b'x'), MAX_MESSAGE_BYTES as i32 + 1, false),
            (Some(0xff), 1, false),
        ];
        for (index, (byte, length, success)) in cases.into_iter().enumerate() {
            let mut owned = OwnedDotnetString::new(&api);
            owned.value = DotnetString {
                ptr: byte.map_or(std::ptr::null(), |byte| Box::into_raw(Box::new(byte))),
                length,
            };
            let copied = owned.into_bytes();
            assert_eq!(FREED.load(Ordering::SeqCst), index + 1);
            assert_eq!(copied.is_ok(), success);
            if length == 1 && success {
                assert_eq!(copied.unwrap(), b"x");
            }
        }
        drop(OwnedDotnetString::new(&api));
        assert_eq!(FREED.load(Ordering::SeqCst), cases.len() + 1);
        for (offset, status) in [
            NativeStatus::Failure as i32,
            NativeStatus::AllocationFailure as i32,
            999,
        ]
        .into_iter()
        .enumerate()
        {
            let result = NativeResult {
                status,
                response: DotnetString {
                    ptr: Box::into_raw(Box::new(b'x')),
                    length: 1,
                },
            };
            assert!(result.into_bytes(&api, "test").is_err());
            assert_eq!(FREED.load(Ordering::SeqCst), cases.len() + 2 + offset);
        }
    }

    #[test]
    fn old_dotted_apphost_name_resolves_bootstrap_not_public_api() {
        let root = std::env::temp_dir().join(format!(
            "shoji-bootstrap-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let bootstrap = root.join("ShojiWM.Runtime.dll");
        std::fs::write(&bootstrap, []).unwrap();
        std::fs::write(root.join("ShojiWM.dll"), []).unwrap();
        assert_eq!(
            component_path(&root.join("ShojiWM.Runtime")).unwrap(),
            bootstrap.canonicalize().unwrap()
        );
        assert_eq!(
            component_path(&bootstrap).unwrap(),
            bootstrap.canonicalize().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
