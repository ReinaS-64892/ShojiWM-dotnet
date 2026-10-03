using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using ShojiWM.Runtime;

internal static unsafe class NativeAbiTests
{
    private static void Check(bool value) { if (!value) throw new InvalidOperationException("native ABI assertion"); }

    public static void Strings()
    {
        delegate* unmanaged<DotnetString, void> free = &NativeEntryPoint.FreeDotnetString;
        int size = IntPtr.Size == 8 ? 16 : 8;
        Check(sizeof(RustString) == size && sizeof(DotnetString) == size);
        Check(sizeof(NativeStatus) == 4 && sizeof(NativeResult) == (IntPtr.Size == 8 ? 24 : 12));
        Check(Marshal.OffsetOf<NativeResult>(nameof(NativeResult.Status)) == 0);
        Check(Marshal.OffsetOf<NativeResult>(nameof(NativeResult.Response)) == IntPtr.Size);
        Check(Marshal.OffsetOf<RustString>(nameof(RustString.Ptr)) == 0);
        Check(Marshal.OffsetOf<DotnetString>(nameof(DotnetString.Ptr)) == 0);
        Check(Marshal.OffsetOf<RustString>(nameof(RustString.Length)) == IntPtr.Size);
        Check(Marshal.OffsetOf<DotnetString>(nameof(DotnetString.Length)) == IntPtr.Size);
        foreach (string text in new[] { "", "ASCII", "日本語・é・🙂\0末尾" })
        {
            var bytes = Encoding.UTF8.GetBytes(text);
            string copied;
            fixed (byte* ptr = bytes) copied = new RustString(ptr, bytes.Length).ToManagedString();
            Array.Fill(bytes, (byte)'x'); // The managed copy outlives its borrowed input.
            Check(copied == text);
            var owned = DotnetString.CopyFrom(text);
            var other = DotnetString.CopyFromUtf8(Encoding.UTF8.GetBytes(text));
            try
            {
                Check(owned.Length == Encoding.UTF8.GetByteCount(text));
                Check(new RustString(owned.Ptr, owned.Length).ToManagedString() == text);
                Check(new RustString(other.Ptr, other.Length).ToManagedString() == text);
                Check(text.Length == 0 || owned.Ptr != other.Ptr);
            }
            finally { free(owned); free(other); }
        }
        Check(new RustString(null, 0).ToManagedString() == "");
        foreach (int length in new[] { -1, 1, NativeEntryPoint.MaxMessageBytes + 1 })
            Reject(() => new RustString(null, length).ToManagedString());
        foreach (byte[] invalid in new byte[][] { [0xff], [0xc0, 0xaf], [0xed, 0xa0, 0x80], [0xf0, 0x9f] })
        {
            fixed (byte* ptr = invalid)
            {
                bool rejected = false;
                try { new RustString(ptr, invalid.Length).ToManagedString(); }
                catch (ArgumentException) { rejected = true; }
                Check(rejected);
            }
            Reject(() => DotnetString.CopyFromUtf8(invalid));
        }
        bool oversized = false;
        try { DotnetString.CopyFrom(new string('x', NativeEntryPoint.MaxMessageBytes + 1)); }
        catch (InvalidDataException) { oversized = true; }
        Check(oversized);
        free(default);
    }

    private static void Reject(Action action)
    {
        try { action(); }
        catch (ArgumentException) { return; }
        throw new InvalidOperationException("invalid string accepted");
    }

    private static string ReadAndFree(NativeResult result)
    {
        delegate* unmanaged<DotnetString, void> free = &NativeEntryPoint.FreeDotnetString;
        try { return new RustString(result.Response.Ptr, result.Response.Length).ToManagedString(); }
        finally { free(result.Response); }
    }

    private static string AssertResult(NativeResult result, NativeStatus status)
    {
        string response = ReadAndFree(result);
        Check(result.Status == status);
        return response;
    }

    public static void Run(string config, string request)
    {
        delegate* unmanaged<uint> version = &NativeEntryPoint.GetAbiVersion;
        delegate* unmanaged<RustString, NativeResult> initialize = &NativeEntryPoint.Initialize;
        delegate* unmanaged<RustString, NativeResult> invoke = &NativeEntryPoint.Invoke;
        delegate* unmanaged<NativeResult> shutdown = &NativeEntryPoint.Shutdown;
        Check(version() == NativeEntryPoint.AbiVersion);
        AssertResult(invoke(default), NativeStatus.Failure);
        AssertResult(shutdown(), NativeStatus.Success);
        var path = Encoding.UTF8.GetBytes(config);
        fixed (byte* input = path)
        {
            AssertResult(initialize(new(input, NativeEntryPoint.MaxMessageBytes + 1)), NativeStatus.Failure);
            AssertResult(initialize(new(input, -1)), NativeStatus.Failure);
            AssertResult(initialize(new(null, 1)), NativeStatus.Failure);
            AssertResult(initialize(new(input, path.Length)), NativeStatus.Success);
        }
        try
        {
            fixed (byte* input = path)
                Check(AssertResult(initialize(new(input, path.Length)), NativeStatus.Failure).Contains("already initialized"));
            // Rejected calls from another thread must leave the original host usable.
            Task.Run(() =>
            {
                delegate* unmanaged<RustString, NativeResult> call = &NativeEntryPoint.Invoke;
                delegate* unmanaged<NativeResult> stop = &NativeEntryPoint.Shutdown;
                Check(AssertResult(call(default), NativeStatus.Failure).Contains("different thread"));
                Check(AssertResult(stop(), NativeStatus.Failure).Contains("different thread"));
            }).GetAwaiter().GetResult();
            var json = Encoding.UTF8.GetBytes(request);
            fixed (byte* input = json)
            {
                string copied = AssertResult(invoke(new(input, json.Length)), NativeStatus.Success);
                using var document = JsonDocument.Parse(copied); // Foreign response already freed.
                Check(document.RootElement.GetProperty("requestId").GetUInt64() == 42);
                Check(document.RootElement.GetProperty("ok").GetBoolean());
                AssertResult(invoke(new(input, NativeEntryPoint.MaxMessageBytes + 1)), NativeStatus.Failure);
                AssertResult(invoke(new(input, -1)), NativeStatus.Failure);
            }
            var invalid = new byte[] { 0xff };
            fixed (byte* input = invalid) AssertResult(invoke(new(input, 1)), NativeStatus.Failure);
            AssertResult(invoke(new(null, 1)), NativeStatus.Failure);
        }
        finally { AssertResult(shutdown(), NativeStatus.Success); }
        AssertResult(invoke(default), NativeStatus.Failure);
        AssertResult(shutdown(), NativeStatus.Success);
        // Sequential compositor/test lifetimes can initialize after a clean shutdown.
        fixed (byte* input = path) AssertResult(initialize(new(input, path.Length)), NativeStatus.Success);
        AssertResult(shutdown(), NativeStatus.Success);
    }
}
