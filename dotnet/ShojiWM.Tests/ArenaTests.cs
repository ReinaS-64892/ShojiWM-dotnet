using System.Runtime.InteropServices;
using System.Text.Json;
using ShojiWM.Runtime;
using ShojiWM.Wire;

internal static unsafe class ArenaTests
{
    private static void Check(bool condition) { if (!condition) throw new Exception("arena assertion"); }
    internal static SwmEvaluateInput WriteInput(EvaluateRequest v, ArenaWriter w) => new() {
        Snapshot = w.Put(AbiConvert.WriteWaylandWindowSnapshot(v.Snapshot, w, 0)),
        WindowId = v.WindowId is {} id ? w.Put(w.String(id)) : null,
        Context = new() {
            NowMs = v.Context.NowMs,
            Displays = AbiConvert.WriteWaylandOutputEntrySlice(v.Context.Displays.Select(x => new WaylandOutputEntry(x.Key, x.Value)).ToList(), w, 0),
            Inputs = AbiConvert.WriteRuntimeInputEntrySlice(v.Context.Inputs.Select(x => new RuntimeInputEntry(x.Key, x.Value)).ToList(), w, 0),
        },
    };
    [StructLayout(LayoutKind.Sequential)]
    private struct AlignmentProbe<T> where T : unmanaged { public byte Prefix; public T Value; }
    private static int Alignment<T>() where T : unmanaged {
        var probe = default(AlignmentProbe<T>);
        return checked((int)((byte*)&probe.Value - (byte*)&probe));
    }
    private sealed class SingleCountList : IReadOnlyList<string>
    {
        private int reads;
        public int Count => ++reads == 1 ? 1 : throw new InvalidOperationException("slice count read more than once");
        public string this[int index] => index == 0 ? "owned UTF-8" : throw new IndexOutOfRangeException();
        public IEnumerator<string> GetEnumerator() => throw new NotSupportedException();
        System.Collections.IEnumerator System.Collections.IEnumerable.GetEnumerator() => GetEnumerator();
    }
    public static void Basics()
    {
        Check(IntPtr.Size == 8);
        Check(sizeof(ResultArena) == 16 && sizeof(ArenaString) == 16);
        Check(Alignment<ResultArena>() == 8 && Alignment<ArenaString>() == 8);
        Check(Alignment<SwmEvaluateResult>() == 8 && Alignment<EvaluateResult>() == 8);
        Check(sizeof(SwmEvaluateInput) == 56 && Alignment<SwmEvaluateInput>() == 8);
        Check(sizeof(SwmEvaluatePreviewInput) == 56 && sizeof(SwmEvaluateCandidatePreviewInput) == 56);
        Check(sizeof(SwmEvaluateCachedInput) == 64 && sizeof(SwmInvokeHandlerInput) == 72 && sizeof(SwmEvaluationContext) == 40);
        Check(Marshal.OffsetOf<SwmEvaluationContext>(nameof(SwmEvaluationContext.Displays)) == 8);
        Check(Marshal.OffsetOf<SwmEvaluationContext>(nameof(SwmEvaluationContext.Inputs)) == 24);
        Check(Marshal.OffsetOf<SwmEvaluateInput>(nameof(SwmEvaluateInput.WindowId)) == 8);
        Check(Marshal.OffsetOf<SwmEvaluateInput>(nameof(SwmEvaluateInput.Context)) == 16);
        Check(Marshal.OffsetOf<SwmEvaluatePreviewInput>(nameof(SwmEvaluatePreviewInput.Context)) == 16);
        Check(Marshal.OffsetOf<SwmEvaluateCandidatePreviewInput>(nameof(SwmEvaluateCandidatePreviewInput.Context)) == 16);
        Check(Marshal.OffsetOf<SwmEvaluateCachedInput>(nameof(SwmEvaluateCachedInput.WindowId)) == 8);
        Check(Marshal.OffsetOf<SwmEvaluateCachedInput>(nameof(SwmEvaluateCachedInput.Context)) == 24);
        Check(Marshal.OffsetOf<SwmInvokeHandlerInput>(nameof(SwmInvokeHandlerInput.HandlerId)) == 16);
        Check(Marshal.OffsetOf<SwmInvokeHandlerInput>(nameof(SwmInvokeHandlerInput.Context)) == 32);
        Check(sizeof(SwmStatusResult) == 32 && sizeof(SwmInvokeHandlerResult) == 40 && sizeof(SwmLifecycleEnableResult) == 40 && sizeof(SwmCommitAssemblyResult) == 40);
        Check(sizeof(InvokeHandlerResult) == 32 && sizeof(EnableResult) == 24 && sizeof(CommitResult) == 8);
        Check(sizeof(uint) == 4 && sizeof(int) == 4 && sizeof(byte) == 1 && sizeof(ulong) == 8);
        Check(Marshal.OffsetOf<ResultArena>(nameof(ResultArena.Ptr)) == 0 && Marshal.OffsetOf<ResultArena>(nameof(ResultArena.Length)) == 8);
        Check(Marshal.OffsetOf<ArenaString>(nameof(ArenaString.Ptr)) == 0 && Marshal.OffsetOf<ArenaString>(nameof(ArenaString.Length)) == 8);
        Check(Marshal.OffsetOf<EvaluateResult>(nameof(EvaluateResult.Node)) == 0 && Marshal.OffsetOf<EvaluateResult>(nameof(EvaluateResult.Actions)) == 8);
        Check(Marshal.OffsetOf<InvokeHandlerResult>(nameof(InvokeHandlerResult.Invoked)) == 24);
        Check(Marshal.OffsetOf<EnableResult>(nameof(EnableResult.DebugConfig)) == 16);
        Check(Marshal.OffsetOf<SwmStatusResult>(nameof(SwmStatusResult.Arena)) == 8);
        Check(Marshal.OffsetOf<SwmStatusResult>(nameof(SwmStatusResult.Error)) == 24);
        Check(Marshal.OffsetOf<SwmInvokeHandlerResult>(nameof(SwmInvokeHandlerResult.Value)) == 24);
        Check(Marshal.OffsetOf<SwmInvokeHandlerResult>(nameof(SwmInvokeHandlerResult.Error)) == 32);
        Check(Marshal.OffsetOf<SwmLifecycleEnableResult>(nameof(SwmLifecycleEnableResult.Value)) == 24);
        Check(Marshal.OffsetOf<SwmLifecycleEnableResult>(nameof(SwmLifecycleEnableResult.Error)) == 32);
        Check(Marshal.OffsetOf<SwmCommitAssemblyResult>(nameof(SwmCommitAssemblyResult.Value)) == 24);
        Check(Marshal.OffsetOf<SwmCommitAssemblyResult>(nameof(SwmCommitAssemblyResult.Error)) == 32);
        Check(sizeof(AbiDimension) == 24 && sizeof(AbiClickDescriptor) == 24 && sizeof(AbiFontFamily) == 16 && sizeof(AbiResizeHitArea) == 16);
        Check(sizeof(SwmEvaluateResult) == 40 && sizeof(EvaluateResult) == 24);
        Check(Marshal.OffsetOf<SwmEvaluateResult>(nameof(SwmEvaluateResult.Arena)) == 8);
        Check(Marshal.OffsetOf<SwmEvaluateResult>(nameof(SwmEvaluateResult.Value)) == 24);
        Check(Marshal.OffsetOf<SwmEvaluateResult>(nameof(SwmEvaluateResult.Error)) == 32);
        NativeLayouts.Validate();
        Check(sizeof(RuntimeWindowActionSlice) == 16 && sizeof(StringSlice) == 16);
        foreach (string text in new[] { "", "日本語🙂\0", new string('x', 100000) }) {
            using var measure = new ArenaWriter(); measure.Put(measure.String(text)); measure.Put(7.25);
            using var w = new ArenaWriter(measure.Length);
            var s = w.Put(w.String(text)); var d = w.Put(7.25);
            Check((nuint)s % 8 == 0 && (nuint)d % 8 == 0);
            Check(AbiConvert.ReadString(*s) == text && *d == 7.25);
            var arena = w.Finish(); delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree; free(arena);
        }
        using (var measure = new ArenaWriter()) {
            AbiConvert.WriteStringSlice(new SingleCountList(), measure, 0);
            using var writer = new ArenaWriter(measure.Length);
            var slice = AbiConvert.WriteStringSlice(new SingleCountList(), writer, 0);
            Check(AbiConvert.ReadStringSlice(slice, 0).Single() == "owned UTF-8");
        }
        ArenaWriter.FailAllocationForTest = true;
        try {
            var failure = AbiConvert.FailureEvaluate(NativeFailureStatus.SemanticError, "failure");
            Check(failure.Status == 1 && failure.Arena.Ptr == null && failure.Arena.Length == 0 && failure.Error == null);
        }
        finally { ArenaWriter.FailAllocationForTest = false; }
        try { using var m = new ArenaWriter(); m.String(new string('x', ArenaWriter.Limit + 1)); throw new Exception("oversized arena accepted"); }
        catch (InvalidDataException) { }
    }
    private static EvaluationResult Roundtrip(EvaluationResult reply)
    {
        long before = ArenaWriter.AllocationCount;
        var result = AbiConvert.WriteEvaluate(reply);
        Check(ArenaWriter.AllocationCount == before + 1);
        delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
        try { Check(result.Status == 0); return new(AbiConvert.ReadWireDecorationNode(*result.Value->Node, 0), AbiConvert.ReadRuntimeWindowActionSlice(result.Value->Actions, 0)); }
        finally { free(result.Arena); }
    }
    internal static EvaluationResult WideFixture(int count) => new(
        new() { Kind = "WindowBorder", Children = Enumerable.Range(0, count).Select(i => new WireDecorationNode {
            Kind = "Label", NodeId = i.ToString(), Props = new() {
                Text = "日本語🙂" + new string('x', 128), Id = "label-" + i,
                Style = new() { Width = "auto", Height = 42, Visible = true, Opacity = 0.75f, FontFamily = new(["sans", "日本語"]), Border = new() { Px = 1, Color = "#fff" } },
                Interaction = new() { ResizeHitArea = new(4, 8) },
            },
        }).ToList() },
        Enumerable.Range(0, count).Select(i => new RuntimeWindowAction { WindowId = i.ToString(), Action = WaylandWindowAction.Focus, Channel = "channel" }).ToList());
    public static void Graphs()
    {
        Check(Roundtrip(new(new() { Kind = "Window" }, [])).Actions.Count == 0);
        var failure = AbiConvert.FailureEvaluate(NativeFailureStatus.SemanticError, "設定エラー🙂");
        delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
        try { Check(failure.Status == 1 && failure.Value == null && AbiConvert.ReadString(*failure.Error) == "設定エラー🙂"); }
        finally { free(failure.Arena); }
        foreach (int count in new[] { 1, 64, 512 }) {
            var fixture = WideFixture(count); var actual = Roundtrip(fixture);
            Check(JsonSerializer.Serialize(actual, WireJson.Options) == JsonSerializer.Serialize(fixture, WireJson.Options));
        }
        WireDecorationNode Deep(int depth) => depth == 0 ? new() { Kind = "Window" } : new() { Kind = "Box", Children = [Deep(depth - 1)] };
        Check(Roundtrip(new(Deep(20), [])).Node != null);
        try { AbiConvert.WriteEvaluate(new(Deep(70), [])); throw new Exception("deep graph accepted"); } catch (InvalidDataException) { }
        try { AbiConvert.WriteEvaluate(new(new() { Kind = "Label", Props = new() { Style = new() { Opacity = float.NaN } } }, [])); throw new Exception("NaN accepted"); } catch (InvalidDataException) { }
        using var icon = JsonDocument.Parse("{}");
        try { AbiConvert.WriteEvaluate(new(new() { Kind = "Label", Props = new() { Icon = icon.RootElement } }, [])); throw new Exception("unsupported icon accepted"); } catch (NotSupportedException) { }
        try { new ShojiWM.Box { Children = [BuildDeep(100)] }.ToWire((_, _) => "handler"); throw new Exception("deep composition accepted"); } catch (InvalidOperationException e) { Check(e.Message.Contains("depth")); }
        static ShojiWM.CompositionNode BuildDeep(int depth) => depth == 0 ? new ShojiWM.ClientWindow() : new ShojiWM.Box { Children = [BuildDeep(depth - 1)] };
        foreach (bool invoked in new[] { false, true }) {
            var handler = AbiConvert.WriteInvokeHandler(new(invoked, null, []));
            try { Check(handler.Value->Node == null && handler.Value->Invoked == (invoked ? 1 : 0)); } finally { free(handler.Arena); }
        }
        var enabled = AbiConvert.WriteLifecycleEnable(new([], new() { FpsCounter = true }));
        try { Check(enabled.Value->DebugConfig->FpsCounter == 1); } finally { free(enabled.Arena); }
        var commit = AbiConvert.WriteCommitAssembly(new(null));
        try { Check(commit.Value->DebugConfig == null); } finally { free(commit.Arena); }
        var ack = AbiConvert.WriteAck();
        Check(ack.Status == 0 && ack.Arena.Ptr == null && ack.Arena.Length == 0 && ack.Error == null);
    }
    public static void AllocationFreeAck()
    {
        _ = AbiConvert.WriteAck(); // Warm the method before measuring.
        long nativeAllocations = ArenaWriter.AllocationCount;
        long nativeBytes = ArenaWriter.AllocatedBytes;
        long managedBytes = GC.GetAllocatedBytesForCurrentThread();
        for (int i = 0; i < 1000; i++) {
            var ack = AbiConvert.WriteAck();
            Check(ack.Status == 0 && ack.Arena.Ptr == null && ack.Arena.Length == 0 && ack.Error == null);
        }
        Check(GC.GetAllocatedBytesForCurrentThread() == managedBytes);
        Check(ArenaWriter.AllocationCount == nativeAllocations && ArenaWriter.AllocatedBytes == nativeBytes);
        ArenaWriter.FailAllocationForTest = true;
        try { Check(AbiConvert.WriteAck().Status == 0); }
        finally { ArenaWriter.FailAllocationForTest = false; }
    }
    public static void Differential(EvaluateRequest request, EvaluationResult legacy)
    {
        using var measure = new ArenaWriter(); WriteInput(request, measure);
        using var writer = new ArenaWriter(measure.Length); var input = WriteInput(request, writer);
        delegate* unmanaged<SwmEvaluateInput, SwmEvaluateResult> evaluate = &NativeEntryPoint.SWMEvaluate;
        delegate* unmanaged<ResultArena, void> free = &NativeEntryPoint.SWMArenaFree;
        var result = evaluate(input);
        try {
            Check(result.Status == 0);
            var nativeTree = AbiConvert.ReadWireDecorationNode(*result.Value->Node, 0);
            string Normalize(WireDecorationNode node) => System.Text.RegularExpressions.Regex.Replace(JsonSerializer.Serialize(node, WireJson.Options), "handler-[a-f0-9]+-[0-9]+", "fixture-handler");
            Check(Normalize(nativeTree) == Normalize(legacy.Node!));
            Check(JsonSerializer.Serialize(AbiConvert.ReadRuntimeWindowActionSlice(result.Value->Actions, 0), WireJson.Options) == JsonSerializer.Serialize(legacy.Actions, WireJson.Options));
        } finally { free(result.Arena); }
    }
}
