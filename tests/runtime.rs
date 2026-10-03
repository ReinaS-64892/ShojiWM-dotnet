//! Exercises the real launcher/trait/host boundary without starting a compositor.
use shojiwm_dotnet::DotNetLauncher;
use shojiwm_lib::{
    runtime_api::{
        ConfigRuntime, DecorationRequest, HostMessage, LaunchContext, ReloadPreparation,
        RuntimeEvent, RuntimeHost, RuntimeLauncher, RuntimeReply, RuntimeRequest,
    },
    ssd::{
        DecorationNode, DecorationNodeKind, WaylandWindowAction, WaylandWindowSnapshot,
        WindowAction,
    },
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn await_preparation(host: &RuntimeHost) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        while let Some(message) = host.pop() {
            if let HostMessage::ReloadReady(result) = message {
                return result;
            }
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(125),
            "reload timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn reload(runtime: &mut dyn ConfigRuntime, host: &RuntimeHost) -> Result<(), String> {
    assert!(matches!(
        runtime.prepare_reload().map_err(|e| e.to_string())?,
        ReloadPreparation::Pending
    ));
    await_preparation(host)?;
    runtime.reload().map_err(|e| e.to_string())
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "shoji-runtime-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn label(node: &DecorationNode) -> Option<&str> {
    if let DecorationNodeKind::Label(label) = &node.kind {
        return Some(&label.text);
    }
    node.children.iter().find_map(label)
}
fn handler(node: &DecorationNode) -> Option<&str> {
    if let DecorationNodeKind::Button(button) = &node.kind
        && let WindowAction::RuntimeHandler(id) = &button.action
    {
        return Some(id);
    }
    node.children.iter().find_map(handler)
}
fn request_handler(id: &str) -> RuntimeRequest<'_> {
    RuntimeRequest::Decoration(DecorationRequest::InvokeHandler {
        window_id: "1",
        handler_id: id,
    })
}
fn evaluate(
    runtime: &mut dyn shojiwm_lib::runtime_api::ConfigRuntime,
    window: &WaylandWindowSnapshot,
) -> DecorationNode {
    let reply = runtime
        .request(
            1234.0,
            RuntimeRequest::Decoration(DecorationRequest::Evaluate {
                window,
                preview: false,
            }),
        )
        .unwrap();
    let RuntimeReply::Evaluation(result) = reply else {
        panic!("wrong evaluation reply");
    };
    result.node
}

#[test]
#[ignore = "requires built .NET projects and SHOJI_TEST_DOTNET_RUNTIME / SHOJI_TEST_DOTNET_FIXTURE"]
fn real_launcher_lifecycle_host_reload_rollback_and_shutdown() {
    let bootstrap = PathBuf::from(std::env::var_os("SHOJI_TEST_DOTNET_RUNTIME").unwrap())
        .canonicalize()
        .unwrap();
    let original = PathBuf::from(std::env::var_os("SHOJI_TEST_DOTNET_FIXTURE").unwrap())
        .canonicalize()
        .unwrap();
    let directory = Directory::new();
    let config_dir = directory.0.join("config");
    fs::create_dir(&config_dir).unwrap();
    for entry in fs::read_dir(original.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), config_dir.join(entry.file_name())).unwrap();
        }
    }
    let config = config_dir.join(original.file_name().unwrap());
    let traces = directory.0.join("threads.log");
    let disposal = directory.0.join("disposal.log");
    let settings = |text: &str, failure: &str| {
        fs::write(
            config_dir.join("fixture-settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "text": text, "failure": failure, "threadMarker": traces, "marker": disposal,
            }))
            .unwrap(),
        )
        .unwrap();
    };
    settings("initial", "");
    let host = RuntimeHost::detached();
    let context = LaunchContext {
        config_path: Some(config),
        runtime_dir: bootstrap.parent().map(Path::to_owned),
        dev: false,
        extra: BTreeMap::new(),
        host: host.clone(),
    };
    let mut runtime = DotNetLauncher.launch(context);
    assert!(!traces.exists(), "launcher executed config before preload");
    runtime.preload().unwrap();
    runtime.preload().unwrap();
    assert_eq!(
        fs::read_to_string(&traces).unwrap().lines().count(),
        1,
        "preload called OnEnable or loaded twice"
    );
    assert!(host.pop().is_none());
    runtime.enable().unwrap();
    runtime.enable().unwrap();
    assert!(matches!(host.pop(), Some(HostMessage::Debug(update)) if !update.fps_counter));
    assert!(host.pop().is_none());
    let rect = shojiwm_lib::ssd::WindowPositionSnapshot {
        x: 0.5,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    let window = WaylandWindowSnapshot {
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
    };
    let tree = evaluate(runtime.as_mut(), &window);
    assert_eq!(label(&tree), Some("initial"));
    let old_handler = handler(&tree).unwrap().to_string();
    let reply = runtime
        .request(1235.0, request_handler(&old_handler))
        .unwrap();
    assert!(
        matches!(reply, RuntimeReply::Handler(reply) if reply.invoked && reply.actions.len() == 1 && reply.actions[0].action == WaylandWindowAction::Close)
    );
    let cached = runtime
        .request(
            1236.0,
            RuntimeRequest::Decoration(DecorationRequest::EvaluateCached {
                window_id: "1",
                window: None,
                force_full: false,
            }),
        )
        .unwrap();
    assert!(
        matches!(cached, RuntimeReply::CachedEvaluation(reply) if reply.node.is_none() && reply.actions.is_empty())
    );
    assert!(matches!(
        runtime.request(0.0, RuntimeRequest::SchedulerTick).unwrap(),
        RuntimeReply::Unhandled
    ));
    assert!(matches!(
        runtime
            .request(
                0.0,
                RuntimeRequest::Input(shojiwm_lib::runtime_api::InputRequest::KeyBinding {
                    binding_id: "missing"
                })
            )
            .unwrap(),
        RuntimeReply::Unhandled
    ));
    assert!(matches!(
        runtime
            .request(
                0.0,
                RuntimeRequest::Decoration(DecorationRequest::StartClose { window_id: "1" })
            )
            .unwrap(),
        RuntimeReply::Unhandled
    ));

    for failure in ["constructor", "enable", "render", "tree"] {
        settings("rejected", failure);
        assert!(
            reload(runtime.as_mut(), &host).is_err(),
            "reload accepted {failure}"
        );
        assert!(
            host.pop().is_none(),
            "candidate config escaped before commit"
        );
        assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("initial"));
        assert!(
            matches!(runtime.request(0.0, request_handler(&old_handler)).unwrap(), RuntimeReply::Handler(reply) if reply.invoked)
        );
    }
    settings("after", "");
    reload(runtime.as_mut(), &host).unwrap();
    assert!(matches!(host.pop(), Some(HostMessage::Debug(update)) if update.fps_counter));
    assert!(host.pop().is_none());
    let tree = evaluate(runtime.as_mut(), &window);
    assert_eq!(label(&tree), Some("after"));
    let new_handler = handler(&tree).unwrap().to_string();
    assert_ne!(new_handler, old_handler);
    assert!(
        matches!(runtime.request(0.0, request_handler(&old_handler)).unwrap(), RuntimeReply::Handler(reply) if !reply.invoked)
    );
    assert!(
        matches!(runtime.request(0.0, request_handler(&new_handler)).unwrap(), RuntimeReply::Handler(reply) if reply.invoked)
    );

    // Owned event snapshots survive generation changes and are delivered in the
    // next managed request without retaining borrowed Rust values.
    let output = shojiwm_lib::ssd::WaylandOutputSnapshot {
        name: "test".into(),
        description: None,
        make: None,
        model: None,
        serial: None,
        connector: None,
        enabled: true,
        resolution: None,
        position: shojiwm_lib::ssd::OutputPositionSnapshot { x: 0, y: 0 },
        scale: 1.0,
        transform: Default::default(),
        available_modes: vec![],
        subpixel: Default::default(),
        detected_subpixel: Default::default(),
    };
    let displays = BTreeMap::from([("test".into(), output)]);
    let expected_displays = 1;
    runtime.post(0.0, RuntimeEvent::DisplayState(displays));
    use shojiwm_lib::runtime_input::{RuntimeInputDeviceKindSnapshot, RuntimeInputDeviceSnapshot};
    runtime.post(
        0.0,
        RuntimeEvent::InputState(BTreeMap::from([(
            "keyboard".into(),
            RuntimeInputDeviceSnapshot {
                name: "keyboard".into(),
                sysname: None,
                vendor: None,
                product: None,
                kind: RuntimeInputDeviceKindSnapshot {
                    keyboard: true,
                    pointer: false,
                    touchpad: false,
                    touch: false,
                    tablet_tool: false,
                    tablet_pad: false,
                    gesture: false,
                    switch_device: false,
                },
            },
        )])),
    );
    settings("environment", "");
    reload(runtime.as_mut(), &host).unwrap();
    host.pop();
    assert_eq!(
        label(&evaluate(runtime.as_mut(), &window)),
        Some(format!("displays:{expected_displays};inputs:1").as_str())
    );
    let reply = runtime
        .request(
            0.0,
            RuntimeRequest::Decoration(DecorationRequest::Closed { window_id: "1" }),
        )
        .unwrap();
    assert!(matches!(reply, RuntimeReply::Done));
    runtime.shutdown();
    runtime.shutdown();
    assert!(runtime.reload().is_err());
    drop(runtime);
    let trace = fs::read_to_string(&traces).unwrap();
    let ids: std::collections::HashSet<_> = trace
        .lines()
        .map(|line| line.split(':').nth(1).unwrap())
        .collect();
    assert_eq!(ids.len(), 1, "config callbacks changed managed thread");
    for operation in [
        "constructor",
        "enable",
        "render",
        "handler",
        "disable",
        "dispose",
    ] {
        assert!(
            trace.lines().any(|line| line.starts_with(operation)),
            "missing {operation}"
        );
    }
    let disposed = fs::read_to_string(disposal).unwrap();
    assert!(
        disposed.contains("dispose:initial")
            && disposed.contains("dispose:after")
            && disposed.contains("dispose:environment")
    );
}

#[test]
#[ignore = "requires .NET 10 SDK and SHOJI_TEST_DOTNET_RUNTIME"]
fn project_build_reload_keeps_active_config_on_build_failure() {
    let directory = Directory::new();
    let project = directory.0.join("Config.csproj");
    let api = Path::new(env!("CARGO_MANIFEST_DIR")).join("dotnet/ShojiWM/ShojiWM.csproj");
    let gate = directory.0.join("hold-build");
    let pid = directory.0.join("build-pid");
    let script = directory.0.join("gate.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nwhile test -f '{}'; do sleep 0.1; done\n",
            pid.display(),
            gate.display()
        ),
    )
    .unwrap();
    fs::write(&project, format!(r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup><ItemGroup><ProjectReference Include="{}" /></ItemGroup><Target Name="TestGate" BeforeTargets="CoreCompile" Condition="Exists('{}')"><Exec Command="sh '{}'" /></Target></Project>"#, api.display(), gate.display(), script.display())).unwrap();
    fs::write(
        directory.0.join("NuGet.Config"),
        "<configuration><packageSources><clear /></packageSources></configuration>",
    )
    .unwrap();
    let source = directory.0.join("Config.cs");
    let config_source = |text: &str| {
        format!(
            r#"using ShojiWM;
public sealed class Config : IWindowConfig {{
    public CompositionNode RenderWindow(WaylandWindow w, RenderContext c) =>
        new WindowBorder {{ Children = [new Label {{ Text = "{text}" }}, new ClientWindow()] }};
}}"#
        )
    };
    fs::write(&source, config_source("before")).unwrap();
    let bootstrap = PathBuf::from(std::env::var_os("SHOJI_TEST_DOTNET_RUNTIME").unwrap())
        .canonicalize()
        .unwrap();
    let host = RuntimeHost::detached();
    let context = LaunchContext {
        config_path: Some(directory.0.join("Config.dll")),
        runtime_dir: bootstrap.parent().map(Path::to_owned),
        dev: true,
        extra: BTreeMap::from([("dotnet-project".into(), project.to_str().unwrap().into())]),
        host: host.clone(),
    };
    let mut runtime = DotNetLauncher.launch(context);
    runtime.preload().unwrap();
    runtime.enable().unwrap();
    let window = WaylandWindowSnapshot {
        id: "1".into(),
        title: "test".into(),
        app_id: None,
        position: Default::default(),
        rect: Default::default(),
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
    };
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    fs::write(&source, "not valid C#").unwrap();
    assert!(matches!(
        runtime.prepare_reload().unwrap(),
        ReloadPreparation::Pending
    ));
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    assert!(
        await_preparation(&host)
            .unwrap_err()
            .contains("build failed")
    );
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    fs::write(&source, config_source("after")).unwrap();
    fs::write(&gate, "hold").unwrap();
    assert!(matches!(
        runtime.prepare_reload().unwrap(),
        ReloadPreparation::Pending
    ));
    assert!(matches!(
        runtime.prepare_reload().unwrap(),
        ReloadPreparation::Pending
    ));
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pid.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        runtime
            .reload()
            .unwrap_err()
            .to_string()
            .contains("still running")
    );
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    fs::remove_file(&gate).unwrap();
    await_preparation(&host).unwrap();
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("before"));
    runtime.reload().unwrap();
    assert!(host.pop().is_none(), "duplicate reload notification");
    assert_eq!(label(&evaluate(runtime.as_mut(), &window)), Some("after"));
    // Cancel a real blocked compiler, including its MSBuild Exec descendant.
    fs::remove_file(&pid).unwrap();
    fs::write(&gate, "hold").unwrap();
    runtime.prepare_reload().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pid.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let child = fs::read_to_string(&pid).unwrap();
    let start = std::time::Instant::now();
    runtime.shutdown();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    // Orphans can briefly be zombies until init reaps them, but must not run.
    let path = format!("/proc/{}/stat", child.trim());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match fs::read_to_string(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Ok(stat) if stat.rsplit_once(") ").unwrap().1.starts_with('Z') => break,
            _ => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "build descendant remained running"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    assert!(host.pop().is_none());
    // Drop is also a shutdown boundary, even if the caller omits shutdown().
    drop(runtime);
}
