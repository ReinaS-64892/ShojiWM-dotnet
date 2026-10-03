using System.Text;
using System.Text.Json;
using System.Runtime.CompilerServices;
using ShojiWM;
using ShojiWM.Example;
using ShojiWM.Runtime;
using ShojiWM.Wire;

// BCL-only executable test harness: any failed assertion makes the process fail.
var passed = 0;
void Test(string name, Action test)
{
    test();
    Console.WriteLine($"PASS {name}");
    passed++;
}
void Check(bool condition, string message = "assertion failed")
{
    if (!condition) throw new InvalidOperationException(message);
}
void Throws(Action action)
{
    try { action(); }
    catch (JsonException) { return; }
    throw new InvalidOperationException("expected JsonException");
}
string Json<T>(T value) => JsonSerializer.Serialize(value, WireJson.Options);
T Parse<T>(string value) => JsonSerializer.Deserialize<T>(value, WireJson.Options)!;
var fixture = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "window.json"));
var snapshot = Parse<WaylandWindowSnapshot>(fixture);
if (args.Contains("--measure")) { ArenaMeasurements.Run(snapshot); return; }
var context = new EvaluationContext(1234, new Dictionary<string, WaylandOutputSnapshot>(), new Dictionary<string, RuntimeInputDeviceSnapshot>());
EvaluateRequest Evaluate(WaylandWindowSnapshot? window = null) => new(window ?? snapshot, (window ?? snapshot).Id, context);
EvaluatePreviewRequest Preview(WaylandWindowSnapshot? window = null) => new(window ?? snapshot, (window ?? snapshot).Id, context);
EvaluateCandidatePreviewRequest CandidatePreview() => new(snapshot, snapshot.Id, context);
EvaluateCachedRequest Cached() => new(snapshot, snapshot.Id, context);
InvokeHandlerRequest Invoke(string handler, string window = "1") => new(window, handler, context);
string Failure(Action call) { try { call(); } catch (Exception e) { return e.Message; } throw new Exception("operation unexpectedly succeeded"); }
IEnumerable<WireDecorationNode> Nodes(WireDecorationNode node) => new[] { node }.Concat(node.Children.SelectMany(Nodes));

Test("generated snapshot roundtrip and enum spellings", () =>
{
    var again = Parse<WaylandWindowSnapshot>(Json(snapshot));
    Check(again.Title == "Kitty 日本語" && again.AppId == "kitty" && again.IsFocused);
    Check(again.Rect.X == 0.5 && again.Decoration.Mode == WindowDecorationModeSnapshot.Server);
    Check(Json(WindowDecorationProtocolSnapshot.KdeServerDecoration) == "\"kde-server-decoration\"");
    Check(Json(OutputTransformSnapshot.Rotate90) == "\"rotate-90\"");
    Check(Json(OutputSubpixelSnapshot.HorizontalRgb) == "\"horizontal-rgb\"");
    Check(Json(WaylandWindowAction.ScheduleAnimation) == "\"scheduleAnimation\"");
    Throws(() => Parse<WindowDecorationModeSnapshot>("\"bogus\""));
    Throws(() => Parse<WindowDecorationModeSnapshot>("7"));
});

Test("optional and null properties; Rust icon bytes remain number arrays", () =>
{
    var noApp = Parse<WaylandWindowSnapshot>(fixture.Replace("\"appId\": \"kitty\",", ""));
    Check(noApp.AppId is null && !Json(noApp).Contains("appId"));
    Check(Parse<WaylandWindowSnapshot>(fixture.Replace("\"kitty\"", "null")).AppId is null);
    Check(Parse<WireStyle>("{}").Width is null && Parse<WireStyle>("{\"width\":null}").Width is null);
    var icon = new WindowIconSnapshot { Bytes = [0, 128, 255] };
    Check(Json(icon) == "{\"bytes\":[0,128,255]}");
});

Test("composition, styles and untagged unions roundtrip", () =>
{
    var style = Parse<WireStyle>(Json(new Style { Width = 28, FontFamily = "sans", Border = new() { Px = 1, Color = "#fff" } }));
    Check(style.Width?.Pixels == 28 && style.FontFamily?.Names[0] == "sans");
    Check(Parse<Dimension>("\"unsupported-keyword\"").Keyword == "unsupported-keyword");
    var action = Parse<ClickDescriptor>(Json(new ClickDescriptor(WireWindowAction.Close)));
    Check(action.Action == WireWindowAction.Close);
    Check(Parse<ClickDescriptor>(Json(new ClickDescriptor("handler-42"))).Handler?.Id == "handler-42");
    Throws(() => Parse<ClickDescriptor>("{\"kind\":\"wrong\",\"id\":\"1\"}"));
    using var session = new RuntimeSession(new ExampleConfig());
    var tree = Parse<WireDecorationNode>(Json(session.Evaluate(Evaluate(snapshot)).Node));
    Check(tree.Kind == "WindowBorder" && Nodes(tree).Count(node => node.Kind == "Window") == 1);
    Check(Nodes(tree).Single(node => node.Kind == "Label").Props.Text!.Contains(snapshot.Title));
});

Test("callback IDs stable across render and isolated from preview/window close", () =>
{
    using var session = new RuntimeSession(new ExampleConfig());
    string Handler(WireDecorationNode tree) => Nodes(tree).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
    var id = Handler(session.Evaluate(Evaluate(snapshot)).Node!);
    Check(id == Handler(session.EvaluateCached(Cached()).Node!));
    session.EvaluatePreview(Preview(snapshot));
    var invoked = session.InvokeHandler(Invoke(id));
    Check(invoked.Invoked == true && invoked.Node is not null);
    Check(invoked.Actions.Single().Action == WaylandWindowAction.Close && invoked.Actions[0].WindowId == snapshot.Id);
    Check(session.InvokeHandler(Invoke("unknown")).Invoked == false);
    session.WindowClosed(new("1"));
    Check(session.InvokeHandler(Invoke(id)).Invoked == false);
});

Test("preview delegates do not replace live delegates; removed handlers are released", () =>
{
    var config = new HandlerConfig();
    using var session = new RuntimeSession(config);
    var tree = session.Evaluate(Evaluate(snapshot)).Node!;
    var handler = Nodes(tree).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
    var preview = session.EvaluatePreview(Preview(snapshot));
    Check(preview.Actions.Count == 0);
    session.InvokeHandler(Invoke(handler));
    Check(config.LiveClicks == 1 && config.PreviewClicks == 0);
    config.ShowButton = false;
    session.Evaluate(Evaluate(snapshot));
    Check(session.InvokeHandler(Invoke(handler)).Invoked == false);
});

Test("handler IDs cannot invoke another window's delegate", () =>
{
    using var session = new RuntimeSession(new ExampleConfig());
    var another = Parse<WaylandWindowSnapshot>(fixture.Replace("\"id\": \"1\"", "\"id\": \"2\""));
    var tree = session.Evaluate(Evaluate(snapshot)).Node!;
    var id = Nodes(tree).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
    session.Evaluate(Evaluate(another));
    var response = session.InvokeHandler(Invoke(id, "2"));
    Check(response.Invoked == false && response.Actions.Count == 0);
});

Test("native arena ABI, ownership, size guards and exception containment", () => NativeAbiTests.Run(typeof(ExampleConfig).Assembly.Location, Evaluate(snapshot)));
Test("arena layout, Unicode, alignment and allocation failure", ArenaTests.Basics);
Test("payload-free success allocates no managed or native memory", ArenaTests.AllocationFreeAck);
Test("native graphs: wide/deep trees, actions, optional values and unsupported data", ArenaTests.Graphs);

Test("config assembly loading shares API identity", () =>
{
    var loader = new ConfigLoader(typeof(ExampleConfig).Assembly.Location);
    Check(loader.CreateConfig() is IWindowConfig);
    loader.Unload();
});

Test("lifecycle enable/disable called exactly once per generation", () =>
{
    var config = new LifecycleConfig();
    using var session = new RuntimeSession(config);
    session.LifecycleEnable(new("initial"));
    session.LifecycleEnable(new("initial"));
    session.LifecycleDisable(new("shutdown"));
    session.LifecycleDisable(new("shutdown"));
    Check(config.Enables == 1 && config.Disables == 1);
});

Test("handler IDs do not cross runtime generations", () =>
{
    using var oldSession = new RuntimeSession(new HandlerConfig());
    using var nextSession = new RuntimeSession(new HandlerConfig());
    var oldTree = oldSession.Evaluate(Evaluate(snapshot)).Node!;
    var nextTree = nextSession.Evaluate(Evaluate(snapshot)).Node!;
    var oldHandler = Nodes(oldTree).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
    var nextHandler = Nodes(nextTree).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
    Check(oldHandler != nextHandler);
    Check(nextSession.InvokeHandler(Invoke(oldHandler)).Invoked == false);
    Check(nextSession.InvokeHandler(Invoke(nextHandler)).Invoked == true);
});

Test("collectible assembly reload releases handlers, tasks, timer, thread, event and GCHandle", () =>
{
    var marker = Path.GetTempFileName();
    Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_MARKER", marker);
    Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_TEXT", "original");
    try {
        using var host = new ConfigurationHost(typeof(ReloadFixtureConfig).Assembly.Location);
        host.LifecycleEnable(new("initial"));
        var first = host.Evaluate(Evaluate()).Node;
        var oldHandler = Nodes(first).Single(node => node.Kind == "Button").Props.OnClick!.Handler!.Id;
        for (int i = 0; i < 20; i++) {
            var text = $"generation-{i}";
            Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_TEXT", text);
            host.PrepareAssembly(new(typeof(ReloadFixtureConfig).Assembly.Location));
            var preview = host.EvaluateCandidatePreview(CandidatePreview());
            Check(Nodes(preview.Node).Single(node => node.Kind == "Label").Props.Text == text);
            Check(host.InvokeHandler(Invoke(oldHandler)).Invoked == (i == 0));
            host.CommitAssembly();
            Check(host.PendingUnloadCount == 0 && host.UnloadedCount == i + 1);
            Check(Nodes(host.Evaluate(Evaluate()).Node).Single(node => node.Kind == "Label").Props.Text == text);
            Check(!host.InvokeHandler(Invoke(oldHandler)).Invoked);
        }
        host.ShutdownAssemblies(); host.Dispose();
        Check(host.PendingUnloadCount == 0 && host.UnloadedCount == 21);
        var lines = File.ReadAllLines(marker);
        Check(lines.Count(line => line.StartsWith("dispose:")) == 21);
        Check(lines.Count(line => line.StartsWith("disable:")) == 21);
    } finally {
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_MARKER", null);
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_TEXT", null);
        File.Delete(marker);
    }
});

Test("missing, broken, wrong entry and managed failures preserve active assembly and allow retry", () =>
{
    var broken = Path.GetTempFileName(); File.WriteAllText(broken, "not an assembly");
    Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_TEXT", "retained");
    try {
        using var host = new ConfigurationHost(typeof(ReloadFixtureConfig).Assembly.Location);
        host.LifecycleEnable(new("initial"));
        void Prepare(string path) => host.PrepareAssembly(new(path));
        foreach (var path in new[] { broken + ".missing", broken, typeof(IWindowConfig).Assembly.Location }) {
            Failure(() => Prepare(path)); Check(host.PendingUnloadCount == 0);
        }
        foreach (var failure in new[] { "constructor", "enable", "render" }) {
            Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", failure);
            if (failure == "render") {
                Prepare(typeof(ReloadFixtureConfig).Assembly.Location);
                Check(Failure(() => host.EvaluateCandidatePreview(CandidatePreview())).Contains(failure));
                host.AbortAssembly();
            } else Check(Failure(() => Prepare(typeof(ReloadFixtureConfig).Assembly.Location)).Contains(failure));
            Check(host.PendingUnloadCount == 0, $"failed {failure} ALC leaked");
            Check(Nodes(host.Evaluate(Evaluate()).Node).Single(node => node.Kind == "Label").Props.Text == "retained");
        }
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", "disable");
        Prepare(typeof(ReloadFixtureConfig).Assembly.Location); host.CommitAssembly();
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", null);
        Prepare(typeof(ReloadFixtureConfig).Assembly.Location); host.CommitAssembly();
        Check(host.PendingUnloadCount == 0);
        Failure(() => host.CommitAssembly()); Failure(() => host.EvaluateCandidatePreview(CandidatePreview()));
    } finally {
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", null);
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_TEXT", null);
        File.Delete(broken);
    }
});

[MethodImpl(MethodImplOptions.NoInlining)]
void ClearLeakedSubscription()
{
    var cleanup = (Action)AppDomain.CurrentDomain.GetData("ShojiWM.TestCleanup")!;
    AppDomain.CurrentDomain.SetData("ShojiWM.TestCleanup", null); cleanup();
}
Test("unload failure is diagnosed and prevents accumulating leaked generations", () =>
{
    try {
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", "leak");
        using var host = new ConfigurationHost(typeof(ReloadFixtureConfig).Assembly.Location);
        host.LifecycleEnable(new("initial"));
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", null);
        void Prepare() => host.PrepareAssembly(new(typeof(ReloadFixtureConfig).Assembly.Location));
        Prepare(); host.CommitAssembly(); Check(host.PendingUnloadCount == 1);
        Check(Failure(Prepare).StartsWith("unload:") && host.PendingUnloadCount == 1);
        host.Evaluate(Evaluate()); ClearLeakedSubscription(); host.VerifyUnloads();
        Check(host.PendingUnloadCount == 0); Prepare(); host.AbortAssembly(); Check(host.PendingUnloadCount == 0);
    } finally {
        Environment.SetEnvironmentVariable("SHOJI_TEST_CONFIG_FAILURE", null);
        if (AppDomain.CurrentDomain.GetData("ShojiWM.TestCleanup") is not null) ClearLeakedSubscription();
    }
});
Test("candidate config delta is buffered until commit and discarded on abort", () =>
{
    using var host = new ConfigurationHost(typeof(ReloadFixtureConfig).Assembly.Location);
    Check(host.LifecycleEnable(new("initial")).DebugConfig is not null);
    host.PrepareAssembly(new(typeof(ReloadFixtureConfig).Assembly.Location)); host.AbortAssembly();
    host.PrepareAssembly(new(typeof(ReloadFixtureConfig).Assembly.Location));
    var committed = host.CommitAssembly(); Check(committed.DebugConfig is not null);
    var roundtrip = Parse<AssemblyCommitResult>(Json(committed));
    Check(roundtrip.DebugConfig is not null && roundtrip.DebugConfig.FpsCounter == committed.DebugConfig!.FpsCounter);
});

Console.WriteLine($"{passed} tests passed");

sealed class LifecycleConfig : IWindowConfig
{
    public int Enables { get; private set; }
    public int Disables { get; private set; }
    public void OnEnable(string reason) => Enables++;
    public void OnDisable(string reason) => Disables++;
    public CompositionNode RenderWindow(WaylandWindow window, RenderContext context) => new ClientWindow();
}

sealed class HandlerConfig : IWindowConfig
{
    public int LiveClicks { get; private set; }
    public int PreviewClicks { get; private set; }
    public bool ShowButton { get; set; } = true;
    public CompositionNode RenderWindow(WaylandWindow window, RenderContext context)
    {
        if (context.IsPreview) window.Close(); // Preview commands are suppressed.
        return new WindowBorder
        {
            Children = ShowButton
                ? [new ClientWindow(), new Button { OnClick = () => { if (context.IsPreview) PreviewClicks++; else LiveClicks++; } }]
                : [new ClientWindow()],
        };
    }
}
