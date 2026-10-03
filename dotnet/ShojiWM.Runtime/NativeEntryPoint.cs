using System.Runtime.InteropServices;
using System.Text.Json;
using ShojiWM.Wire;

namespace ShojiWM.Runtime;

/// Permanent bootstrap exports for a single compositor-owned host. Initialize
/// and Shutdown bracket its lifetime; reload only replaces configuration ALCs.
/// RustString inputs are borrowed; all result strings use FreeDotnetString.
public static unsafe class NativeEntryPoint
{
    public const int MaxMessageBytes = 8 * 1024 * 1024;
    public const uint AbiVersion = 3;

    private static ConfigurationHost? host;
    private static int ownerThreadId;
    private static int inCall;

    [UnmanagedCallersOnly]
    public static uint GetAbiVersion() => AbiVersion;

    [UnmanagedCallersOnly]
    public static NativeResult Initialize(RustString path)
    {
        if (!TryEnter()) return Busy();
        try
        {
            if (host is not null) throw new InvalidOperationException("host already initialized");
            // Publish only after a successful load; failure permits a later retry.
            host = new ConfigurationHost(path.ToManagedString());
            ownerThreadId = Environment.CurrentManagedThreadId;
            return Success();
        }
        catch (Exception exception) { return Failure(exception); }
        finally { Exit(); }
    }

    [UnmanagedCallersOnly]
    public static NativeResult Invoke(RustString request)
    {
        if (!TryEnter()) return Busy();
        try
        {
            var current = CurrentHost();
            var json = request.ToManagedString();
            try
            {
                var result = current.HandleJson(json);
                return Success(DotnetString.CopyFromUtf8(JsonSerializer.SerializeToUtf8Bytes(result, WireJson.Options)));
            }
            catch (Exception exception)
            {
                // Excessive output rejects a candidate without disabling the host.
                ulong id = 0;
                var kind = "protocolError";
                using var document = JsonDocument.Parse(json);
                if (document.RootElement.TryGetProperty("requestId", out var value)) value.TryGetUInt64(out id);
                if (document.RootElement.TryGetProperty("kind", out value) && value.ValueKind == JsonValueKind.String) kind = value.GetString()!;
                var failure = new ExternalRuntimeResponse { RequestId = id, Kind = kind, Ok = false, Error = Diagnostic(exception), Actions = [] };
                return Success(DotnetString.CopyFromUtf8(JsonSerializer.SerializeToUtf8Bytes(failure, WireJson.Options)));
            }
        }
        catch (Exception exception) { return Failure(exception); }
        finally { Exit(); }
    }

    [UnmanagedCallersOnly]
    public static NativeResult Shutdown()
    {
        if (!TryEnter()) return Busy();
        try
        {
            if (host is null) return Success(); // Idempotent after shutdown or a failed initialization.
            var current = CurrentHost();
            host = null;
            ownerThreadId = 0;
            current.Dispose();
            return Success();
        }
        catch (Exception exception) { return Failure(exception); }
        finally { Exit(); }
    }

    [UnmanagedCallersOnly]
    public static void FreeDotnetString(DotnetString value)
    {
        try { DotnetString.Free(value); }
        catch (Exception) { /* No managed exception may cross even this void export. */ }
    }

    // Reject concurrent and reentrant calls rather than waiting inside the FFI.
    // The successful entrant alone owns all accesses to host and ownerThreadId.
    private static bool TryEnter() => Interlocked.CompareExchange(ref inCall, 1, 0) == 0;
    private static void Exit() => Volatile.Write(ref inCall, 0);
    private static NativeResult Busy()
    {
        try { return new(NativeStatus.Failure, DotnetString.CopyFrom("concurrent or reentrant host call")); }
        catch { return new(NativeStatus.AllocationFailure, default); }
    }

    private static ConfigurationHost CurrentHost()
    {
        var current = host ?? throw new InvalidOperationException("host is not initialized");
        if (ownerThreadId != Environment.CurrentManagedThreadId) throw new InvalidOperationException("host called from a different thread");
        return current;
    }

    private static NativeResult Success(DotnetString response = default) => new(NativeStatus.Success, response);
    private static NativeResult Failure(Exception error)
    {
        // Flatten exceptions; never retain config exception/reflection roots.
        try { return new(NativeStatus.Failure, DotnetString.CopyFrom(Diagnostic(error))); }
        catch { return new(NativeStatus.AllocationFailure, default); }
    }

    private static string Diagnostic(Exception error)
    {
        var message = error.Message;
        return message.Length <= 4096 ? message : message[..4096];
    }
}
