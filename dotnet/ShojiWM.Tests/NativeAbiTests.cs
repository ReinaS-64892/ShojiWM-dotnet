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

    public static void Run(string config, string request)
    {
        delegate* unmanaged<uint> version = &NativeEntryPoint.GetAbiVersion;
        delegate* unmanaged<RustString, nint*, DotnetString*, int> create = &NativeEntryPoint.CreateHost;
        delegate* unmanaged<nint, RustString, DotnetString*, int> invoke = &NativeEntryPoint.Invoke;
        delegate* unmanaged<nint, DotnetString*, int> destroy = &NativeEntryPoint.DestroyHost;
        delegate* unmanaged<DotnetString, void> free = &NativeEntryPoint.FreeDotnetString;
        Check(version() == NativeEntryPoint.AbiVersion);
        var path = Encoding.UTF8.GetBytes(config);
        nint handle = 0;
        DotnetString response = default;
        fixed (byte* input = path) Check(create(new(input, path.Length), &handle, &response) == 0 && handle != 0 && response.Ptr == null);
        try
        {
            var json = Encoding.UTF8.GetBytes(request);
            fixed (byte* input = json)
            {
                Check(invoke(handle, new(input, json.Length), &response) == 0);
                string copied;
                try { copied = new RustString(response.Ptr, response.Length).ToManagedString(); }
                finally { free(response); }
                using var document = JsonDocument.Parse(copied); // No dependency on the freed allocation.
                Check(document.RootElement.GetProperty("requestId").GetUInt64() == 42);
                Check(document.RootElement.GetProperty("ok").GetBoolean());
                Check(invoke(handle, new(input, NativeEntryPoint.MaxMessageBytes + 1), &response) != 0);
                free(response);
                Check(invoke(handle, new(input, -1), &response) != 0);
                free(response);
            }
            var invalid = new byte[] { 0xff };
            fixed (byte* input = invalid)
            {
                Check(invoke(handle, new(input, 1), &response) != 0);
                free(response);
            }
            Check(invoke(handle, new(null, 1), &response) != 0);
            free(response);
            Check(invoke(0, default, &response) != 0);
            free(response);
            Check(invoke(handle, default, null) != 0);
        }
        finally { Check(destroy(handle, &response) == 0); free(response); }
        fixed (byte* input = path)
        {
            Check(create(new(input, NativeEntryPoint.MaxMessageBytes + 1), &handle, &response) != 0 && handle == 0);
            free(response);
            Check(create(new(input, path.Length), null, &response) != 0);
        }
        free(default);
    }
}
