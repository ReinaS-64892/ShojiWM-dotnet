using System.Text;
using System.Text.Json;
using ShojiWM.Runtime;
using ShojiWM.Wire;

internal static unsafe class NativeAbiTests
{
    private static void Check(bool value) { if (!value) throw new InvalidOperationException("native ABI assertion"); }
    private static string Consume(SwmEvaluateResult result, int status)
    {
        delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
        try { Check(result.Status == status); return result.Error == null ? "" : AbiConvert.ReadString(*result.Error); }
        finally { free(result.Arena); }
    }
    private static string Consume(SwmStatusResult result, int status)
    {
        delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
        try { Check(result.Status == status); return result.Error == null ? "" : AbiConvert.ReadString(*result.Error); }
        finally { free(result.Arena); }
    }
    public static void Run(string config, EvaluateRequest request)
    {
        delegate* unmanaged<uint> version = &NativeEntryPoint.GetAbiVersion;
        delegate* unmanaged<ArenaString, SwmStatusResult> initialize = &NativeEntryPoint.SWMInitialize;
        delegate* unmanaged<SwmEvaluateInput, SwmEvaluateResult> evaluate = &NativeEntryPoint.SWMEvaluate;
        delegate* unmanaged<SwmStatusResult> shutdown = &NativeEntryPoint.SWMShutdown;
        Check(version() == 6);
        var hostError = Consume(evaluate(default), 2);
        Check(hostError.Contains("System.InvalidOperationException") && hostError.Contains("NativeEntryPoint.CurrentHost"));
        Consume(shutdown(), 0);
        var path = Encoding.UTF8.GetBytes(config);
        fixed (byte* p = path) {
            Consume(initialize(new() { Ptr = null, Length = 1 }), 2);
            Consume(initialize(new() { Ptr = p, Length = -1 }), 2);
            Consume(initialize(new() { Ptr = p, Length = path.Length }), 0);
        }
        try {
            fixed (byte* p = path) Check(Consume(initialize(new() { Ptr = p, Length = path.Length }), 2).Contains("already initialized"));
            Task.Run(() => {
                delegate* unmanaged<SwmEvaluateInput, SwmEvaluateResult> call = &NativeEntryPoint.SWMEvaluate;
                delegate* unmanaged<SwmStatusResult> stop = &NativeEntryPoint.SWMShutdown;
                Check(Consume(call(default), 2).Contains("different thread"));
                Check(Consume(stop(), 2).Contains("different thread"));
            }).GetAwaiter().GetResult();
            // Test-only legacy JSON transport oracle; production has no JSON entry point.
            using var oracle = new ConfigurationHost(config);
            var baseline = JsonSerializer.Deserialize<EvaluationResult>(JsonSerializer.Serialize(oracle.Evaluate(request), WireJson.Options), WireJson.Options)!;
            ArenaTests.Differential(request, baseline);
            Check(Consume(evaluate(default), 4).Contains("System.ArgumentException"));
            // Every exception uses ToString(), including exceptions formerly given
            // a special allocation status. No exception type changes its category.
            try { throw new OutOfMemoryException("test diagnostic"); }
            catch (Exception error) {
                string diagnostic = Consume(AbiConvert.FailureEvaluate(error, NativeFailureStatus.SemanticError), 1);
                Check(diagnostic.Contains("System.OutOfMemoryException") && diagnostic.Contains("NativeAbiTests.Run"));
            }
            using var measure = new ArenaWriter(); ArenaTests.WriteInput(request, measure);
            using var writer = new ArenaWriter(measure.Length);
            var input = ArenaTests.WriteInput(request, writer);
            ArenaWriter.FailAllocationForTest = true;
            try {
                Consume(evaluate(default), 4); // Diagnostic allocation failure retains input category.
                Consume(evaluate(input), 1); // Result allocation failure retains runtime category.
            }
            finally { ArenaWriter.FailAllocationForTest = false; }
            ArenaTests.Differential(request, baseline);
        } finally { Consume(shutdown(), 0); }
        Consume(evaluate(default), 2); Consume(shutdown(), 0);
        fixed (byte* p = path) Consume(initialize(new() { Ptr = p, Length = path.Length }), 0);
        Consume(shutdown(), 0);
    }
}
