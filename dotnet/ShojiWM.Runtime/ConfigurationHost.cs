using System.Runtime.CompilerServices;
using ShojiWM.Wire;

namespace ShojiWM.Runtime;

/// Sequential assembly transaction manager. The native ABI and file
/// watcher are independent; only semantic commands reach this host.
public sealed class ConfigurationHost : IDisposable
{
    private ConfigurationGeneration? current;
    private ConfigurationGeneration? candidate;
    private RuntimeDebugConfigUpdate? candidateDebugConfig;
    private readonly List<WeakReference> retired = [];
    public int PendingUnloadCount => retired.Count;
    public int UnloadedCount { get; private set; }

    public ConfigurationHost(string configPath) => current = ConfigurationGeneration.Load(configPath, retired);

    private RuntimeSession CurrentSession => current?.Session ?? throw new ObjectDisposedException(nameof(ConfigurationHost));
    private RuntimeSession CandidateSession => candidate?.Session ?? throw new ConfigurationException("reload", "no prepared assembly");

    public EvaluationResult Evaluate(EvaluateRequest request) => CurrentSession.Evaluate(request);
    public EvaluationResult EvaluatePreview(EvaluatePreviewRequest request) => CurrentSession.EvaluatePreview(request);
    public EvaluationResult EvaluateCached(EvaluateCachedRequest request) => CurrentSession.EvaluateCached(request);
    public EvaluationResult EvaluateCandidatePreview(EvaluateCandidatePreviewRequest request)
    {
        _ = CurrentSession;
        return CandidateSession.EvaluateCandidatePreview(request);
    }
    public HandlerInvocationResult InvokeHandler(InvokeHandlerRequest request) => CurrentSession.InvokeHandler(request);
    public void WindowClosed(WindowClosedRequest request) => CurrentSession.WindowClosed(request);
    public LifecycleEnableResult LifecycleEnable(LifecycleEnableRequest request) => CurrentSession.LifecycleEnable(request);
    public void LifecycleDisable(LifecycleDisableRequest request) => CurrentSession.LifecycleDisable(request);
    public void DrainPreload() => CurrentSession.DrainPreload();

    public void PrepareAssembly(PrepareAssemblyRequest request)
    {
        _ = CurrentSession;
        if (retired.Count != 0) {
            VerifyUnloads();
            if (retired.Count != 0) throw new ConfigurationException("unload", "previous ALC still referenced; refusing to accumulate generations");
        }
        // Flatten config exceptions in a separate frame before collecting ALCs.
        var error = PrepareAssemblyCore(request);
        VerifyUnloads();
        if (error is not null) throw new ConfigurationException("initialization", error);
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private string? PrepareAssemblyCore(PrepareAssemblyRequest request)
    {
        try {
            ReleaseCandidate();
            candidate = ConfigurationGeneration.Load(request.ConfigPath, retired);
            var enabled = candidate.Session.LifecycleEnable(new("reload"));
            if (enabled.Actions.Count != 0) throw new ConfigurationException("initialization", "OnEnable returned unsupported window actions");
            candidateDebugConfig = enabled.DebugConfig;
            return null;
        } catch (Exception error) {
            var message = error.ToString();
            ReleaseCandidate();
            return message;
        }
    }
    public AssemblyCommitResult CommitAssembly()
    {
        _ = CurrentSession;
        var debug = candidateDebugConfig;
        SwitchGeneration();
        VerifyUnloads();
        return new(debug);
    }
    public void AbortAssembly()
    {
        _ = CurrentSession;
        ReleaseCandidate();
        VerifyUnloads();
    }
    public void ShutdownAssemblies()
    {
        _ = CurrentSession;
        ReleaseAll();
        VerifyUnloads();
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private void SwitchGeneration()
    {
        var next = candidate ?? throw new ConfigurationException("reload", "no prepared assembly");
        candidate = null;
        candidateDebugConfig = null;
        var previous = current;
        current = next;
        if (previous is not null) retired.Add(previous.Release("reload"));
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private void ReleaseCandidate()
    {
        candidateDebugConfig = null;
        var previous = candidate;
        candidate = null;
        if (previous is not null) retired.Add(previous.Release("reload-rejected"));
    }

    public void VerifyUnloads()
    {
        // Bounded GC at reload/shutdown only. Leaking config Tasks/statics
        // cannot force an unbounded wait or silently masquerade as unloaded.
        for (var attempt = 0; retired.Any(weak => weak.IsAlive) && attempt < 3; attempt++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();
        }
        UnloadedCount += retired.RemoveAll(weak => !weak.IsAlive);
        if (retired.Count != 0)
            Console.Error.WriteLine($"ShojiWM .NET unload: {retired.Count} ALC(s) still referenced; config must dispose tasks, timers and external subscriptions");
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private void ReleaseAll()
    {
        ReleaseCandidate();
        var previous = current;
        current = null;
        if (previous is not null) retired.Add(previous.Release("shutdown"));
    }

    public void Dispose()
    {
        ReleaseAll();
        VerifyUnloads();
    }

}
