using ShojiWM.Wire;

namespace ShojiWM.Runtime;

// These are operation inputs, not transport envelopes. Context is shared data,
// without an operation discriminator or unrelated optional payloads.
public sealed record EvaluationContext(ulong NowMs,
    IReadOnlyDictionary<string, WaylandOutputSnapshot> Displays,
    IReadOnlyDictionary<string, RuntimeInputDeviceSnapshot> Inputs)
{
    public static EvaluationContext Empty { get; } = new(0,
        new Dictionary<string, WaylandOutputSnapshot>(), new Dictionary<string, RuntimeInputDeviceSnapshot>());
}
public sealed record EvaluateRequest(WaylandWindowSnapshot Snapshot, string? WindowId, EvaluationContext Context);
public sealed record EvaluatePreviewRequest(WaylandWindowSnapshot Snapshot, string? WindowId, EvaluationContext Context);
public sealed record EvaluateCandidatePreviewRequest(WaylandWindowSnapshot Snapshot, string? WindowId, EvaluationContext Context);
public sealed record EvaluateCachedRequest(WaylandWindowSnapshot? Snapshot, string WindowId, EvaluationContext Context);
public sealed record InvokeHandlerRequest(string WindowId, string HandlerId, EvaluationContext Context);
public sealed record WindowClosedRequest(string WindowId);
public sealed record PrepareAssemblyRequest(string ConfigPath);
public sealed record LifecycleEnableRequest(string Reason);
public sealed record LifecycleDisableRequest(string Reason);

public sealed record EvaluationResult(WireDecorationNode Node, IReadOnlyList<RuntimeWindowAction> Actions);
public sealed record HandlerInvocationResult(bool Invoked, WireDecorationNode? Node, IReadOnlyList<RuntimeWindowAction> Actions);
public sealed record LifecycleEnableResult(IReadOnlyList<RuntimeWindowAction> Actions, RuntimeDebugConfigUpdate? DebugConfig);
public sealed record AssemblyCommitResult(RuntimeDebugConfigUpdate? DebugConfig);
