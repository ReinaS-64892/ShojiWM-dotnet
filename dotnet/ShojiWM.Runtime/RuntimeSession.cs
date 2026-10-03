using ShojiWM.Wire;

namespace ShojiWM.Runtime;

/// Owns managed callbacks and cached snapshots. Each operation enters its own
/// method; neither semantic nor transport operation dispatch happens here.
public sealed class RuntimeSession : IDisposable
{
    private IWindowConfig? config;
    public RuntimeSession(IWindowConfig config) => this.config = config;
    private sealed record WindowEntry(WaylandWindowSnapshot Snapshot, Dictionary<string, (string Id, Action Callback)> Handlers);
    private readonly Dictionary<string, WindowEntry> windows = [];
    private readonly List<RuntimeWindowAction> actions = [];
    private ulong nextHandlerId;
    private readonly string generationId = Guid.NewGuid().ToString("N");
    private bool enabled;

    private IWindowConfig RequireConfig() => config ?? throw new ObjectDisposedException(nameof(RuntimeSession));

    public EvaluationResult Evaluate(EvaluateRequest request)
    {
        actions.Clear();
        try { return new(Render(ValidateSnapshot(request.Snapshot, request.WindowId), request.Context, false), [.. actions]); }
        finally { actions.Clear(); }
    }
    public EvaluationResult EvaluatePreview(EvaluatePreviewRequest request)
    {
        actions.Clear();
        try { return new(Render(ValidateSnapshot(request.Snapshot, request.WindowId), request.Context, true), [.. actions]); }
        finally { actions.Clear(); }
    }
    public EvaluationResult EvaluateCandidatePreview(EvaluateCandidatePreviewRequest request)
    {
        actions.Clear();
        try { return new(Render(ValidateSnapshot(request.Snapshot, request.WindowId), request.Context, true), [.. actions]); }
        finally { actions.Clear(); }
    }
    public EvaluationResult EvaluateCached(EvaluateCachedRequest request)
    {
        actions.Clear();
        try {
            var snapshot = request.Snapshot ??
                (windows.TryGetValue(request.WindowId, out var cached) ? cached.Snapshot : null)
                ?? throw new InvalidOperationException("evaluate requires a snapshot or a known windowId");
            return new(Render(ValidateSnapshot(snapshot, request.WindowId), request.Context, false), [.. actions]);
        } finally { actions.Clear(); }
    }
    public HandlerInvocationResult InvokeHandler(InvokeHandlerRequest request)
    {
        actions.Clear();
        try {
            RequireConfig();
            if (windows.TryGetValue(request.WindowId, out var entry)) {
                var handler = entry.Handlers.Values.FirstOrDefault(handler => handler.Id == request.HandlerId);
                if (handler.Callback is not null) {
                    handler.Callback();
                    return new(true, Render(entry.Snapshot, request.Context, false), [.. actions]);
                }
            }
            return new(false, null, []);
        } finally { actions.Clear(); }
    }
    public void WindowClosed(WindowClosedRequest request)
    {
        actions.Clear();
        try { var current = RequireConfig(); if (windows.Remove(request.WindowId)) current.OnWindowClosed(request.WindowId); }
        finally { actions.Clear(); }
    }
    public LifecycleEnableResult LifecycleEnable(LifecycleEnableRequest request)
    {
        actions.Clear();
        try {
            var current = RequireConfig();
            if (!enabled) { enabled = true; current.OnEnable(request.Reason); }
            return new([.. actions], current.DebugConfig);
        } finally { actions.Clear(); }
    }
    public void LifecycleDisable(LifecycleDisableRequest request)
    {
        RequireConfig();
        Disable(request.Reason);
    }
    public void DrainPreload() { RequireConfig(); actions.Clear(); }

    private static WaylandWindowSnapshot ValidateSnapshot(WaylandWindowSnapshot snapshot, string? windowId)
    {
        if (string.IsNullOrEmpty(snapshot.Id) || windowId is not null && windowId != snapshot.Id)
            throw new InvalidOperationException("snapshot id must match windowId");
        return snapshot;
    }
    private WireDecorationNode Render(WaylandWindowSnapshot snapshot, EvaluationContext request, bool preview)
    {
        var current = RequireConfig();
        var context = new RenderContext(request.NowMs, preview, request.Displays, request.Inputs);
        var window = new WaylandWindow(snapshot, action => { if (!preview) actions.Add(action); });
        var composition = current.RenderWindow(window, context) ?? throw new InvalidOperationException("config returned no composition");
        var handlers = new Dictionary<string, (string Id, Action Callback)>();
        windows.TryGetValue(snapshot.Id, out var previous);
        var tree = composition.ToWire((key, callback) => {
            var id = previous is not null && previous.Handlers.TryGetValue(key, out var prior)
                ? prior.Id : $"handler-{generationId}-{checked(++nextHandlerId)}";
            handlers.Add(key, (id, callback));
            return id;
        });
        if (!preview) windows[snapshot.Id] = new(snapshot, handlers);
        return tree;
    }
    private void Disable(string reason)
    {
        try { if (enabled) config?.OnDisable(reason); }
        finally { enabled = false; windows.Clear(); actions.Clear(); }
    }
    public void Dispose()
    {
        var previous = config;
        try { Disable("shutdown"); }
        finally {
            config = null;
            if (previous is IAsyncDisposable asyncDisposable) asyncDisposable.DisposeAsync().AsTask().GetAwaiter().GetResult();
            else if (previous is IDisposable disposable) disposable.Dispose();
        }
    }
}
