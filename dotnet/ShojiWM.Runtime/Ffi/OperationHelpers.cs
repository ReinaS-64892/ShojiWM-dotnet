using ShojiWM.Wire;

namespace ShojiWM.Runtime;

public static unsafe partial class AbiConvert
{
    private static WaylandWindowSnapshot RequiredSnapshot(AbiWaylandWindowSnapshot* snapshot) => snapshot == null ? throw new ArgumentException("snapshot pointer is required") : ReadWaylandWindowSnapshot(*snapshot, 0);
    private static string? OptionalString(ArenaString* v) => v == null ? null : ReadString(*v);
    internal static EvaluationContext ReadContext(SwmEvaluationContext v) => new(v.NowMs,
        ReadWaylandOutputEntrySlice(v.Displays, 0).ToDictionary(x => x.Key, x => x.Value),
        ReadRuntimeInputEntrySlice(v.Inputs, 0).ToDictionary(x => x.Key, x => x.Value));

    private struct BuiltArena<T> where T : unmanaged { public ResultArena Arena; public T* Root; }
    private static BuiltArena<TNative> WriteGraph<TManaged, TNative>(TManaged value, Func<TManaged, ArenaWriter, TNative> build) where TNative : unmanaged
    {
        using var measure = new ArenaWriter(); measure.Put(build(value, measure));
        using var writer = new ArenaWriter(measure.Length);
        var root = writer.Put(build(value, writer));
        return new() { Root = root, Arena = writer.Finish() };
    }
    // Format diagnostics without changing the caller's failure category.
    internal static SwmStatusResult FailureStatus(Exception error, NativeFailureStatus status)
    {
        try { return FailureStatus(status, error.ToString()); }
        catch { return FailureStatus(status, "exception diagnostic could not be formatted"); }
    }
    internal static SwmStatusResult WriteAck() => default;
    internal static SwmStatusResult FailureStatus(NativeFailureStatus status, string message)
    {
        try {
            message = message.Length > 4096 ? message[..4096] : message;
            using var measure = new ArenaWriter(); measure.Put(measure.String(message));
            using var writer = new ArenaWriter(measure.Length); var error = writer.Put(writer.String(message));
            return new() { Status = (int)status, Error = error, Arena = writer.Finish() };
        } catch { return new() { Status = (int)status }; }
    }
}
