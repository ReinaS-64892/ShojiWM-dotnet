#[path = "../../src/bridge/host.rs"]
mod host;
use host::InProcessDotNetHost;
use serde_json::json;
#[path = "../../src/bridge/arena.rs"]
mod arena;
#[path = "../../src/bridge/native.rs"]
mod native;
#[path = "../../src/bridge/native_generated.rs"]
mod native_generated;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn snapshot() -> shojiwm_lib::ssd::WaylandWindowSnapshot {
    use shojiwm_lib::ssd::{WaylandWindowSnapshot, window_model::WindowPositionSnapshot};
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
fn context() -> native::EvaluationContext {
    native::EvaluationContext {
        now_ms: 1234,
        ..Default::default()
    }
}
fn evaluate(
    host: &Arc<Mutex<InProcessDotNetHost>>,
) -> Result<native::EvaluationResponse, native::BridgeError> {
    host.lock().unwrap().evaluate(native::EvaluateRequest {
        snapshot: snapshot(),
        window_id: Some("1".into()),
        context: context(),
    })
}
fn candidate_preview(
    host: &Arc<Mutex<InProcessDotNetHost>>,
) -> Result<native::EvaluationResponse, native::BridgeError> {
    host.lock()
        .unwrap()
        .evaluate_candidate_preview(native::EvaluateCandidatePreviewRequest {
            snapshot: snapshot(),
            window_id: Some("1".into()),
            context: context(),
        })
}
fn prepare(
    host: &Arc<Mutex<InProcessDotNetHost>>,
    path: &PathBuf,
) -> Result<(), native::BridgeError> {
    host.lock()
        .unwrap()
        .prepare_assembly(path.to_str().unwrap().to_owned())
}
fn invoke(
    host: &Arc<Mutex<InProcessDotNetHost>>,
    id: &str,
) -> Result<native::HandlerResponse, native::BridgeError> {
    host.lock()
        .unwrap()
        .invoke_handler(native::InvokeHandlerRequest {
            window_id: "1".into(),
            handler_id: id.into(),
            context: context(),
        })
}
fn child(
    tree: &shojiwm_lib::ssd::WireDecorationNode,
    index: usize,
) -> &shojiwm_lib::ssd::WireDecorationNode {
    match &tree.children[index] {
        shojiwm_lib::ssd::bridge::WireDecorationChild::Node(n) => n,
        _ => panic!("primitive unsupported"),
    }
}
fn handler(tree: &shojiwm_lib::ssd::WireDecorationNode) -> Option<&str> {
    if let Some(shojiwm_lib::ssd::bridge::WireOnClick::RuntimeHandler(h)) = &tree.props.on_click {
        return Some(&h.id);
    }
    tree.children.iter().find_map(|c| match c {
        shojiwm_lib::ssd::bridge::WireDecorationChild::Node(n) => handler(n),
        _ => None,
    })
}
fn env(name: &str, value: &str) {
    // Test fixture environment is changed only while no managed callback is
    // executing; fixture backgrounds do not read these variables.
    unsafe {
        std::env::set_var(name, value);
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let runtime = PathBuf::from(&args[1]);
    let original = PathBuf::from(&args[2]);
    let root = std::env::temp_dir().join(format!(
        "shoji-native-test-日本語-🙂-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let staging = root.join("config");
    std::fs::create_dir(&staging).unwrap();
    for entry in std::fs::read_dir(original.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), staging.join(entry.file_name())).unwrap();
        }
    }
    let fixture = staging.join(original.file_name().unwrap());
    let settings = |text: &str, failure: &str| {
        std::fs::write(
            staging.join("fixture-settings.json"),
            serde_json::to_vec(&json!({"text":text,"failure":failure})).unwrap(),
        )
        .unwrap()
    };
    settings("initial", "");
    let threads = root.join("threads.log");
    let disposal = root.join("disposal.log");
    env("SHOJI_TEST_THREAD_MARKER", threads.to_str().unwrap());
    env("SHOJI_TEST_CONFIG_MARKER", disposal.to_str().unwrap());
    let text = "日本語・é・🙂\0末尾";
    settings(text, "");
    assert!(InProcessDotNetHost::start(&runtime, &root.join("missing.dll")).is_err());
    let host = Arc::new(Mutex::new(
        InProcessDotNetHost::start(&runtime.with_extension(""), &fixture).unwrap(),
    ));
    // Reject a second owner without shutting down or replacing the active singleton.
    let duplicate = InProcessDotNetHost::start(&runtime, &fixture).unwrap_err();
    assert!(duplicate.contains("already initialized"), "{duplicate}");
    let enabled = host
        .lock()
        .unwrap()
        .lifecycle_enable("initial".into())
        .unwrap();
    assert!(enabled.debug_config.is_some());
    let first = evaluate(&host).unwrap();
    assert_eq!(child(&first.node, 0).props.text.as_deref().unwrap(), text);
    let preview = host
        .lock()
        .unwrap()
        .evaluate_preview(native::EvaluatePreviewRequest {
            snapshot: snapshot(),
            window_id: Some("1".into()),
            context: context(),
        })
        .unwrap();
    assert!(preview.actions.is_empty());
    let cached = host
        .lock()
        .unwrap()
        .evaluate_cached(native::EvaluateCachedRequest {
            snapshot: None,
            window_id: "1".into(),
            context: context(),
        })
        .unwrap();
    assert_eq!(
        child(&cached.node, 0).props.text,
        child(&first.node, 0).props.text
    );
    assert!(host.lock().unwrap().drain_preload().is_ok());
    let mut previous = handler(&first.node).unwrap().to_owned();
    let mut ids = HashSet::from([previous.clone()]);
    for generation in 0..20 {
        settings(&format!("generation-{generation}"), "");
        let clone = host.clone();
        let path = fixture.clone();
        // Reload called from a different Rust thread still enters the same
        // managed host thread. No native delegate points into the config ALC.
        std::thread::spawn(move || {
            assert!(prepare(&clone, &path).is_ok());
            assert!(candidate_preview(&clone).is_ok());
            assert!(clone.lock().unwrap().commit_assembly().is_ok());
        })
        .join()
        .unwrap();
        assert!(invoke(&host, &previous).unwrap().invoked == false);
        let tree = evaluate(&host).unwrap();
        assert_eq!(
            child(&tree.node, 0).props.text.as_deref().unwrap(),
            format!("generation-{generation}")
        );
        previous = handler(&tree.node).unwrap().into();
        assert!(ids.insert(previous.clone()));
        let action = invoke(&host, &previous).unwrap();
        assert!(action.invoked);
        assert_eq!(
            action.actions[0].action,
            shojiwm_lib::ssd::WaylandWindowAction::Close
        );
    }
    // Size guards replace pipe-line limits, including oversized managed output.
    settings(&"x".repeat(arena::MAX_ARENA_SIZE + 1), "");
    assert!(prepare(&host, &fixture).is_ok());
    let oversized = candidate_preview(&host);
    assert!(!oversized.is_ok());
    assert!(oversized.unwrap_err().to_string().contains("8 MiB"));
    assert!(host.lock().unwrap().abort_assembly().is_ok());
    for failure in ["constructor", "enable", "render"] {
        settings("candidate", failure);
        let prepare = prepare(&host, &fixture);
        if failure == "render" {
            assert!(prepare.is_ok());
            assert!(candidate_preview(&host).is_err());
            assert!(host.lock().unwrap().abort_assembly().is_ok());
        } else {
            assert!(prepare.is_err());
        }
        assert!(evaluate(&host).is_ok());
    }
    settings("accepted", "");
    let broken = root.join("broken.dll");
    std::fs::write(&broken, b"broken bytes").unwrap();
    for path in [broken, root.join("missing.dll"), runtime.clone()] {
        assert!(prepare(&host, &path).is_err());
        assert!(evaluate(&host).is_ok());
    }
    settings("leaked", "leak");
    assert!(prepare(&host, &fixture).is_ok());
    assert!(host.lock().unwrap().commit_assembly().is_ok());
    settings("accepted", "");
    assert!(prepare(&host, &fixture).is_ok());
    assert!(host.lock().unwrap().commit_assembly().is_ok());
    let refusal = prepare(&host, &fixture);
    assert!(refusal.is_err());
    assert!(
        refusal
            .unwrap_err()
            .to_string()
            .contains("still referenced")
    );
    let tree = evaluate(&host).unwrap();
    let id = handler(&tree.node).unwrap();
    assert!(invoke(&host, id).is_ok());
    assert!(prepare(&host, &fixture).is_ok());
    assert!(host.lock().unwrap().abort_assembly().is_ok());
    host.lock().unwrap().window_closed("1".into()).unwrap();
    assert!(!invoke(&host, id).unwrap().invoked);
    host.lock()
        .unwrap()
        .lifecycle_disable("test".into())
        .unwrap();
    assert!(host.lock().unwrap().shutdown_assemblies().is_ok());
    drop(host);
    let trace = std::fs::read_to_string(threads).unwrap();
    let lines: Vec<_> = trace.lines().collect();
    let thread_ids: HashSet<_> = lines
        .iter()
        .map(|line| line.split(':').nth(1).unwrap())
        .collect();
    assert_eq!(
        thread_ids.len(),
        1,
        "callbacks crossed managed threads: {trace}"
    );
    for operation in [
        "constructor",
        "enable",
        "render",
        "handler",
        "disable",
        "dispose",
    ] {
        assert!(lines.iter().any(|line| line.starts_with(operation)));
    }
    let disposed = std::fs::read_to_string(disposal).unwrap();
    assert!(
        disposed
            .lines()
            .filter(|line| line.starts_with("dispose:"))
            .count()
            >= 27
    );
    // No worker is launched by production hosting (including from CLR threads).
    for entry in std::fs::read_dir("/proc/self/task").unwrap() {
        let children = std::fs::read_to_string(entry.unwrap().path().join("children"));
        if let Ok(children) = children {
            assert!(
                children.trim().is_empty(),
                "hosting spawned a child: {children}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    println!(
        "PASS hostfxr bootstrap, API identity, UTF-8 arena ABI, handlers/actions, 20 reloads, rollback, leak/refusal/retry, buffers and single managed thread"
    );
}
