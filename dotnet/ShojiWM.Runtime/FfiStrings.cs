using System.Buffers;
using System.Runtime.InteropServices;
using System.Text;

namespace ShojiWM.Runtime;

/// <summary>
/// Immutable UTF-8 borrowed from Rust for one FFI call. Length is a byte count,
/// not a UTF-16 character count; no NUL terminator is required. Never free Ptr.
/// Copy with ToManagedString before retaining text beyond the call.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public readonly unsafe struct RustString(byte* ptr, int length)
{
    public readonly byte* Ptr = ptr;
    public readonly int Length = length;

    public string ToManagedString()
    {
        if (Length < 0 || Length > NativeEntryPoint.MaxMessageBytes || (Ptr == null && Length != 0))
            throw new ArgumentException("invalid RustString or message exceeds 8 MiB limit");
        var bytes = new ReadOnlySpan<byte>(Ptr, Length);
        FfiStrings.ValidateUtf8(bytes);
        return Encoding.UTF8.GetString(bytes);
    }
}

/// <summary>
/// Immutable UTF-8 allocated by .NET. Returning it transfers ownership to Rust,
/// which must call FreeDotnetString exactly once, including on error paths.
/// Do not free it with Rust's allocator, mutate it, or retain aliases after free.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public readonly unsafe struct DotnetString
{
    public readonly byte* Ptr;
    public readonly int Length;

    private DotnetString(byte* ptr, int length) { Ptr = ptr; Length = length; }

    /// <summary>Allocate a UTF-8 copy independent of the managed string's lifetime.</summary>
    public static DotnetString CopyFrom(string text)
    {
        int length = Encoding.UTF8.GetByteCount(text);
        var result = Allocate(length);
        try
        {
            Encoding.UTF8.GetBytes(text.AsSpan(), new Span<byte>(result.Ptr, length));
            return result;
        }
        catch { Free(result); throw; }
    }

    /// <summary>Copy serialized UTF-8 into memory owned by the native recipient.</summary>
    public static DotnetString CopyFromUtf8(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length > NativeEntryPoint.MaxMessageBytes) throw new InvalidDataException("response exceeds 8 MiB message limit");
        FfiStrings.ValidateUtf8(bytes);
        var result = Allocate(bytes.Length);
        bytes.CopyTo(new Span<byte>(result.Ptr, result.Length));
        return result;
    }

    private static DotnetString Allocate(int length)
    {
        if (length > NativeEntryPoint.MaxMessageBytes) throw new InvalidDataException("response exceeds 8 MiB message limit");
        if (length == 0) return default;
        var ptr = (byte*)NativeMemory.Alloc((nuint)length);
        if (ptr == null) throw new OutOfMemoryException();
        return new(ptr, length);
    }

    /// <summary>
    /// Release an owned allocation exactly once. All aliases become invalid;
    /// the empty default value is allowed. Called by the native free export.
    /// </summary>
    public static void Free(DotnetString value) => NativeMemory.Free(value.Ptr);
}

internal static class FfiStrings
{
    internal static void ValidateUtf8(ReadOnlySpan<byte> bytes)
    {
        // Encoding.UTF8 uses replacement fallback. Validate first to preserve
        // the ABI's rejection of malformed UTF-8 without a separate encoding instance.
        while (!bytes.IsEmpty)
        {
            if (Rune.DecodeFromUtf8(bytes, out _, out int consumed) != OperationStatus.Done)
                throw new ArgumentException("invalid UTF-8 string");
            bytes = bytes[consumed..];
        }
    }
}
