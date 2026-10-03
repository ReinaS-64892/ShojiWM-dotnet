using System.Runtime.InteropServices;
using ShojiWM.Wire;

namespace ShojiWM.Runtime;

/// Permanent bootstrap exports for a single compositor-owned host. Initialize
/// and Shutdown bracket its lifetime; reload only replaces configuration ALCs.
/// Inputs are borrowed for one call; result graphs belong to one arena.
public static unsafe partial class NativeEntryPoint
{
    public const uint AbiVersion = 6;

    private static ConfigurationHost? host;
    private static int ownerThreadId;
    private static int inCall;

    [UnmanagedCallersOnly]
    public static uint GetAbiVersion() => AbiVersion;

    [UnmanagedCallersOnly]
    public static SwmStatusResult SWMInitialize(ArenaString path)
    {
        if (!TryEnter()) return AbiConvert.FailureStatus(NativeFailureStatus.HostError, "concurrent or reentrant host call");
        try {
            if (host is not null) return AbiConvert.FailureStatus(NativeFailureStatus.HostError, "host already initialized");
            var loaded = new ConfigurationHost(AbiConvert.ReadString(path));
            // Allocate the acknowledgement before publishing ownership.
            SwmStatusResult ack;
            try { ack = AbiConvert.WriteAck(); }
            catch { loaded.Dispose(); throw; }
            host = loaded; ownerThreadId = Environment.CurrentManagedThreadId;
            return ack;
        } catch (Exception e) { return AbiConvert.FailureStatus(e, NativeFailureStatus.HostError); }
        finally { Exit(); }
    }
    [UnmanagedCallersOnly]
    public static SwmStatusResult SWMShutdown()
    {
        if (!TryEnter()) return AbiConvert.FailureStatus(NativeFailureStatus.HostError, "concurrent or reentrant host call");
        try {
            if (host is not null) {
                var current = CurrentHost(); host = null; ownerThreadId = 0; current.Dispose();
            }
            return AbiConvert.WriteAck();
        } catch (Exception e) { return AbiConvert.FailureStatus(e, NativeFailureStatus.HostError); }
        finally { Exit(); }
    }
    [UnmanagedCallersOnly]
    public static void SWMArenaFree(ResultArena arena) => NativeMemory.Free(arena.Ptr);

    // Reject concurrent and reentrant calls rather than waiting inside the FFI.
    // The successful entrant alone owns all accesses to host and ownerThreadId.
    private static bool TryEnter() => Interlocked.CompareExchange(ref inCall, 1, 0) == 0;
    private static void Exit() => Volatile.Write(ref inCall, 0);
    private static ConfigurationHost CurrentHost()
    {
        var current = host ?? throw new InvalidOperationException("host is not initialized");
        if (ownerThreadId != Environment.CurrentManagedThreadId) throw new InvalidOperationException("host called from a different thread");
        return current;
    }

}
