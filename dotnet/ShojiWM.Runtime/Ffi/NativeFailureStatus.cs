namespace ShojiWM.Runtime;

// Managed failure categories only. ABI structs keep their fixed-width int fields.
internal enum NativeFailureStatus : int
{
    SemanticError = 1,
    HostError = 2,
    InvalidArgument = 4,
}
