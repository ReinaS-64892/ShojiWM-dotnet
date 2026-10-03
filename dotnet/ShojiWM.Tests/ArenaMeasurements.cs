using System.Diagnostics;
using System.Text.Json;
using ShojiWM.Runtime;
using ShojiWM.Wire;

internal static unsafe class ArenaMeasurements
{
    // Test-only old transport oracle. Reports response transport and complete
    // managed evaluation costs separately; no performance threshold in CI.
    public static void Run(WaylandWindowSnapshot snapshot)
    {
        Console.WriteLine("measurement: elapsed us/op, managed bytes/op, native allocations/op, native bytes/op (64-bit Release)");
        foreach (int count in new[] { 1, 128, 512 }) {
            var fixture = ArenaTests.WideFixture(count);
            Measure($"JSON response {count} nodes/actions", () => {
                var bytes = JsonSerializer.SerializeToUtf8Bytes(fixture, WireJson.Options);
                _ = JsonSerializer.Deserialize<EvaluationResult>(bytes, WireJson.Options);
            });
            Measure($"native response {count} nodes/actions", () => {
                var result = AbiConvert.WriteEvaluate(fixture);
                delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
                try {
                    _ = AbiConvert.ReadWireDecorationNode(*result.Value->Node, 0);
                    _ = AbiConvert.ReadRuntimeWindowActionSlice(result.Value->Actions, 0);
                } finally { free(result.Arena); }
            });
        }
        var command = new EvaluateRequest(snapshot, snapshot.Id, EvaluationContext.Empty);
        using var session = new RuntimeSession(new ShojiWM.Example.ExampleConfig());
        Measure("JSON managed evaluate", () => {
            var input = JsonSerializer.Deserialize<EvaluateRequest>(JsonSerializer.SerializeToUtf8Bytes(command, WireJson.Options), WireJson.Options)!;
            var response = session.Evaluate(input);
            _ = JsonSerializer.Deserialize<EvaluationResult>(JsonSerializer.SerializeToUtf8Bytes(response, WireJson.Options), WireJson.Options);
        });
        Measure("native managed evaluate", () => {
            using var measure = new ArenaWriter(); ArenaTests.WriteInput(command, measure);
            using var writer = new ArenaWriter(measure.Length);
            var input = ArenaTests.WriteInput(command, writer);
            var response = session.Evaluate(AbiConvert.ReadEvaluate(input));
            var result = AbiConvert.WriteEvaluate(response);
            delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
            try { _ = AbiConvert.ReadWireDecorationNode(*result.Value->Node, 0); }
            finally { free(result.Arena); }
        });
    }
    private static void Measure(string name, Action action)
    {
        for (int i = 0; i < 20; i++) action();
        const int iterations = 200;
        long before = GC.GetAllocatedBytesForCurrentThread(), allocations = ArenaWriter.AllocationCount, nativeBytes = ArenaWriter.AllocatedBytes;
        var timer = Stopwatch.StartNew();
        for (int i = 0; i < iterations; i++) action();
        timer.Stop();
        Console.WriteLine($"{name}: {timer.Elapsed.TotalMicroseconds / iterations:F1} us; {(GC.GetAllocatedBytesForCurrentThread() - before) / iterations} managed bytes; {(ArenaWriter.AllocationCount - allocations) / iterations} native allocations; {(ArenaWriter.AllocatedBytes - nativeBytes) / iterations} native bytes");
    }
}
