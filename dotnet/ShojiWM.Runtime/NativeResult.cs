using System.Runtime.InteropServices;

namespace ShojiWM.Runtime;

// Fixed-width ABI status. Semantic configuration errors remain in the JSON response.
public enum NativeStatus : int
{
    Success = 0,
    Failure = 1,
    AllocationFailure = 2,
}

/// <summary>
/// One FFI return value. Response is either semantic JSON or a failure diagnostic;
/// its ownership always transfers to Rust, regardless of Status.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public readonly struct NativeResult(NativeStatus status, DotnetString response)
{
    public readonly NativeStatus Status = status;
    public readonly DotnetString Response = response;
}
