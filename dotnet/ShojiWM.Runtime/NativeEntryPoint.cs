using System.Runtime.InteropServices;
using System.Text.Json;
using ShojiWM.Wire;

namespace ShojiWM.Runtime;

/// Permanent bootstrap exports. All calls for a handle belong to its creating
/// OS thread. RustString inputs are borrowed for the call; DotnetString outputs
/// transfer ownership to Rust and must use FreeDotnetString, even on failure.
public static unsafe class NativeEntryPoint
{
    public const int MaxMessageBytes = 8 * 1024 * 1024;
    public const uint AbiVersion = 2;

    [UnmanagedCallersOnly]
    public static uint GetAbiVersion() => AbiVersion;
    private sealed class HostOwner(string path)
    {
        public readonly ConfigurationHost Host = new(path);
        public readonly int ThreadId = Environment.CurrentManagedThreadId;
    }

    [UnmanagedCallersOnly]
    public static int CreateHost(RustString path, nint* handle, DotnetString* error)
    {
        if (handle != null) *handle = 0;
        if (error != null) *error = default;
        if (handle == null || error == null) return -1;
        try
        {
            var owner = new HostOwner(path.ToManagedString());
            try { *handle = GCHandle.ToIntPtr(GCHandle.Alloc(owner)); }
            catch { owner.Host.Dispose(); throw; }
            return 0;
        }
        catch (Exception exception) { return Failure(exception, error); }
    }

    [UnmanagedCallersOnly]
    public static int Invoke(nint handle, RustString request, DotnetString* response)
    {
        if (response == null) return -1;
        *response = default;
        try
        {
            var host = Owner(handle).Host;
            var json = request.ToManagedString();
            try
            {
                var result = host.HandleJson(json);
                *response = DotnetString.CopyFromUtf8(JsonSerializer.SerializeToUtf8Bytes(result, WireJson.Options));
            }
            catch (Exception exception)
            {
                // Serialization/size errors are semantic rejection, too. A
                // candidate with excessive output must not disable the host
                // or prevent Rust from aborting it and retaining active config.
                ulong id = 0;
                var kind = "protocolError";
                using var document = JsonDocument.Parse(json);
                if (document.RootElement.TryGetProperty("requestId", out var value)) value.TryGetUInt64(out id);
                if (document.RootElement.TryGetProperty("kind", out value) && value.ValueKind == JsonValueKind.String) kind = value.GetString()!;
                var failure = new ExternalRuntimeResponse { RequestId = id, Kind = kind, Ok = false, Error = Diagnostic(exception), Actions = [] };
                *response = DotnetString.CopyFromUtf8(JsonSerializer.SerializeToUtf8Bytes(failure, WireJson.Options));
            }
            return 0;
        }
        catch (Exception exception) { return Failure(exception, response); }
    }

    [UnmanagedCallersOnly]
    public static int DestroyHost(nint handle, DotnetString* error)
    {
        if (error == null) return -1;
        *error = default;
        try
        {
            var owner = Owner(handle);
            try { owner.Host.Dispose(); }
            finally { GCHandle.FromIntPtr(handle).Free(); }
            return 0;
        }
        catch (Exception exception) { return Failure(exception, error); }
    }

    [UnmanagedCallersOnly]
    public static void FreeDotnetString(DotnetString value)
    {
        try { DotnetString.Free(value); }
        catch (Exception) { /* No managed exception may cross even this void export. */ }
    }

    private static HostOwner Owner(nint handle)
    {
        if (handle == 0) throw new ArgumentException("null host handle");
        var owner = (HostOwner)(GCHandle.FromIntPtr(handle).Target ?? throw new ObjectDisposedException("host"));
        if (owner.ThreadId != Environment.CurrentManagedThreadId) throw new InvalidOperationException("host called from a different thread");
        return owner;
    }

    private static int Failure(Exception error, DotnetString* output)
    {
        // Flatten exceptions here; never retain config exception/reflection roots.
        try { *output = DotnetString.CopyFrom(Diagnostic(error)); }
        catch { return -2; }
        return -1;
    }

    private static string Diagnostic(Exception error)
    {
        var message = error.Message;
        return message.Length <= 4096 ? message : message[..4096];
    }
}
