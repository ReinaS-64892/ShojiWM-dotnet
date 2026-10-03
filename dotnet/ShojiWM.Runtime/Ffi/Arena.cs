using System.Runtime.InteropServices;
using System.Text;
using ShojiWM.Wire;

namespace ShojiWM.Runtime;

[StructLayout(LayoutKind.Sequential)]
public unsafe struct ResultArena { public byte* Ptr; public int Length; }
[StructLayout(LayoutKind.Sequential)]
public unsafe struct ArenaString { public byte* Ptr; public int Length; }
[StructLayout(LayoutKind.Sequential)]
public unsafe struct AbiWaylandOutputEntry { public ArenaString Key; public AbiWaylandOutputSnapshot Value; }
[StructLayout(LayoutKind.Sequential)]
public unsafe struct AbiRuntimeInputEntry { public ArenaString Key; public AbiRuntimeInputDeviceSnapshot Value; }
public sealed record WaylandOutputEntry(string Key, WaylandOutputSnapshot Value);
public sealed record RuntimeInputEntry(string Key, RuntimeInputDeviceSnapshot Value);
[StructLayout(LayoutKind.Sequential)]
public struct AbiDimension { public uint Kind; public int Pixels; public ArenaString Keyword; }
[StructLayout(LayoutKind.Sequential)]
public struct AbiFontFamily { public StringSlice Names; }
[StructLayout(LayoutKind.Sequential)]
public struct AbiClickDescriptor { public uint Kind; public uint Action; public ArenaString Handler; }
[StructLayout(LayoutKind.Sequential)]
public unsafe struct AbiResizeHitArea { public int EdgePx; public int* CornerPx; }

// Two identical traversals: measure first, then one zeroed contiguous allocation.
// All slots use 8-byte alignment (the v6 ABI is explicitly 64-bit).
public sealed unsafe class ArenaWriter : IDisposable
{
    public const int Limit = 8 * 1024 * 1024;
    private byte* memory;
    private int capacity;
    private int offset;
    public bool Measuring => memory == null;
    public int Length => offset;
    internal static bool FailAllocationForTest { get; set; }
    internal static long AllocationCount;
    internal static long AllocatedBytes;
    public ArenaWriter() { }
    public ArenaWriter(int size)
    {
        if (IntPtr.Size != 8) throw new PlatformNotSupportedException("native ABI v6 requires 64-bit pointers");
        if (size <= 0 || size > Limit) throw new InvalidDataException("arena exceeds 8 MiB limit");
        if (FailAllocationForTest) throw new OutOfMemoryException("simulated arena allocation failure");
        memory = (byte*)NativeMemory.AllocZeroed((nuint)size);
        if (memory == null) throw new OutOfMemoryException();
        capacity = size;
        Interlocked.Increment(ref AllocationCount);
        Interlocked.Add(ref AllocatedBytes, size);
    }
    public T* Allocate<T>(int count = 1) where T : unmanaged
    {
        if (count < 0) throw new ArgumentException("negative slice length");
        int start = checked((offset + 7) & ~7);
        int end = checked(start + checked(sizeof(T) * count));
        if (end > Limit || !Measuring && end > capacity) throw new InvalidDataException("arena exceeds 8 MiB limit or graph changed during construction");
        offset = end;
        return Measuring || count == 0 ? null : (T*)(memory + start);
    }
    public T* Put<T>(T value) where T : unmanaged { var p = Allocate<T>(); if (!Measuring) *p = value; return p; }
    public ArenaString String(string value)
    {
        int count = Encoding.UTF8.GetByteCount(value);
        var p = Allocate<byte>(count);
        if (!Measuring) Encoding.UTF8.GetBytes(value, new Span<byte>(p, count));
        return new() { Ptr = p, Length = count };
    }
    public ResultArena Finish()
    {
        if (Measuring || offset != capacity) throw new InvalidOperationException("arena measurement mismatch");
        var arena = new ResultArena { Ptr = memory, Length = capacity };
        memory = null;
        return arena;
    }
    public void Dispose() { NativeMemory.Free(memory); memory = null; }
}

public static unsafe partial class AbiConvert
{
    internal const int MaxDepth = 64;
    internal static void CheckDepth(int depth) { if (depth > MaxDepth) throw new InvalidDataException("native graph maximum depth exceeded"); }
    internal static void CheckSlice(void* ptr, int length) { if (length < 0 || length > ArenaWriter.Limit || ptr == null && length != 0) throw new ArgumentException("invalid input slice"); }
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);
    internal static string ReadString(ArenaString v) { CheckSlice(v.Ptr, v.Length); return StrictUtf8.GetString(new ReadOnlySpan<byte>(v.Ptr, v.Length)); }
    internal static float Finite(float v) => float.IsFinite(v) ? v : throw new InvalidDataException("non-finite ABI number");
    internal static double Finite(double v) => double.IsFinite(v) ? v : throw new InvalidDataException("non-finite ABI number");
    internal static bool ReadBool(byte v) => v switch { 0 => false, 1 => true, _ => throw new ArgumentException("invalid boolean") };
    internal static AbiDimension WriteDimension(Dimension v, ArenaWriter w, int depth) => v.Pixels is int px ? new() { Kind = 0, Pixels = px } : v.Keyword is string keyword ? new() { Kind = 1, Keyword = w.String(keyword) } : throw new ArgumentException("dimension has no value");
    internal static Dimension ReadDimension(AbiDimension v, int depth) => v.Kind switch { 0 => new(v.Pixels), 1 => new(ReadString(v.Keyword)), _ => throw new ArgumentException("invalid dimension tag") };
    internal static AbiFontFamily WriteFontFamily(FontFamily v, ArenaWriter w, int depth) => new() { Names = WriteStringSlice(v.Names, w, depth) };
    internal static FontFamily ReadFontFamily(AbiFontFamily v, int depth) => new(ReadStringSlice(v.Names, depth));
    internal static AbiClickDescriptor WriteClickDescriptor(ClickDescriptor v, ArenaWriter w, int depth) => v.Action is WireWindowAction action ? new() { Kind = 0, Action = WriteWireWindowAction(action) } : v.Handler is { Kind: "runtime-handler" } h ? new() { Kind = 1, Handler = w.String(h.Id) } : throw new ArgumentException("invalid click descriptor");
    internal static ClickDescriptor ReadClickDescriptor(AbiClickDescriptor v, int depth) => v.Kind switch { 0 => new(ReadWireWindowAction(v.Action)), 1 => new(ReadString(v.Handler)), _ => throw new ArgumentException("invalid click tag") };
    internal static AbiResizeHitArea WriteResizeHitArea(ResizeHitArea v, ArenaWriter w, int depth) => new() { EdgePx = v.EdgePx, CornerPx = v.CornerPx is int corner ? w.Put(corner) : null };
    internal static ResizeHitArea ReadResizeHitArea(AbiResizeHitArea v, int depth) => new(v.EdgePx, v.CornerPx == null ? null : *v.CornerPx);
    internal static AbiWaylandOutputEntry WriteWaylandOutputEntry(WaylandOutputEntry v, ArenaWriter w, int depth) => new() { Key = w.String(v.Key), Value = WriteWaylandOutputSnapshot(v.Value, w, depth) };
    internal static WaylandOutputEntry ReadWaylandOutputEntry(AbiWaylandOutputEntry v, int depth) => new(ReadString(v.Key), ReadWaylandOutputSnapshot(v.Value, depth));
    internal static AbiRuntimeInputEntry WriteRuntimeInputEntry(RuntimeInputEntry v, ArenaWriter w, int depth) => new() { Key = w.String(v.Key), Value = WriteRuntimeInputDeviceSnapshot(v.Value, w, depth) };
    internal static RuntimeInputEntry ReadRuntimeInputEntry(AbiRuntimeInputEntry v, int depth) => new(ReadString(v.Key), ReadRuntimeInputDeviceSnapshot(v.Value, depth));
}
