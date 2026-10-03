use super::{arena::*, native_generated::*};
use shojiwm_lib::{
    runtime_debug::RuntimeDebugConfigUpdate,
    runtime_input::RuntimeInputDeviceSnapshot,
    ssd::{RuntimeWindowAction, WaylandOutputSnapshot, WaylandWindowSnapshot, bridge::*},
};
use std::collections::BTreeMap;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiWaylandOutputEntry {
    pub key: ArenaString,
    pub value: AbiWaylandOutputSnapshot,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiRuntimeInputEntry {
    pub key: ArenaString,
    pub value: AbiRuntimeInputDeviceSnapshot,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmEvaluationContext {
    pub now_ms: u64,
    pub displays: Slice<AbiWaylandOutputEntry>,
    pub inputs: Slice<AbiRuntimeInputEntry>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmEvaluateInput {
    pub snapshot: *const AbiWaylandWindowSnapshot,
    pub window_id: *const ArenaString,
    pub context: SwmEvaluationContext,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmEvaluatePreviewInput {
    pub snapshot: *const AbiWaylandWindowSnapshot,
    pub window_id: *const ArenaString,
    pub context: SwmEvaluationContext,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmEvaluateCandidatePreviewInput {
    pub snapshot: *const AbiWaylandWindowSnapshot,
    pub window_id: *const ArenaString,
    pub context: SwmEvaluationContext,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmEvaluateCachedInput {
    pub snapshot: *const AbiWaylandWindowSnapshot,
    pub window_id: ArenaString,
    pub context: SwmEvaluationContext,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwmInvokeHandlerInput {
    pub window_id: ArenaString,
    pub handler_id: ArenaString,
    pub context: SwmEvaluationContext,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EvaluateResult {
    pub node: *const AbiWireDecorationNode,
    pub actions: Slice<AbiRuntimeWindowAction>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct InvokeHandlerResult {
    pub node: *const AbiWireDecorationNode,
    pub actions: Slice<AbiRuntimeWindowAction>,
    pub invoked: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EnableResult {
    pub actions: Slice<AbiRuntimeWindowAction>,
    pub debug_config: *const AbiRuntimeDebugConfigUpdate,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CommitResult {
    pub debug_config: *const AbiRuntimeDebugConfigUpdate,
}

// A shared ownership/status header parametrized by the actual payload type.
// It is never an operation union, and its payload type is fixed by each export.
#[repr(C)]
pub struct NativeResult<T> {
    pub status: i32,
    pub arena: ResultArena,
    pub value: *const T,
    pub error: *const ArenaString,
}
pub type SwmEvaluateResult = NativeResult<EvaluateResult>;
pub type SwmInvokeHandlerResult = NativeResult<InvokeHandlerResult>;
pub type SwmLifecycleEnableResult = NativeResult<EnableResult>;
pub type SwmCommitAssemblyResult = NativeResult<CommitResult>;
#[repr(C)]
pub struct SwmStatusResult {
    pub status: i32,
    pub arena: ResultArena,
    pub error: *const ArenaString,
}
pub type ArenaFree = extern "system" fn(ResultArena);
pub struct OwnedManagedArena {
    arena: ResultArena,
    free: ArenaFree,
}
impl Drop for OwnedManagedArena {
    fn drop(&mut self) {
        (self.free)(self.arena);
    }
}

#[derive(Debug)]
pub enum BridgeError {
    Semantic(String),
    Host(String),
}
impl From<String> for BridgeError {
    fn from(value: String) -> Self {
        Self::Host(value)
    }
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Semantic(s) | Self::Host(s) => s.fmt(f),
        }
    }
}
#[derive(Debug)]
pub struct EvaluationResponse {
    pub node: WireDecorationNode,
    pub actions: Vec<RuntimeWindowAction>,
}
#[derive(Debug)]
pub struct HandlerResponse {
    pub node: Option<WireDecorationNode>,
    pub actions: Vec<RuntimeWindowAction>,
    pub invoked: bool,
}
#[derive(Debug)]
pub struct EnableResponse {
    pub actions: Vec<RuntimeWindowAction>,
    pub debug_config: Option<RuntimeDebugConfigUpdate>,
}
#[derive(Debug)]
pub struct CommitResponse {
    pub debug_config: Option<RuntimeDebugConfigUpdate>,
}

fn status(
    status: i32,
    arena: ResultArena,
    error: *const ArenaString,
) -> Result<Reader, BridgeError> {
    // Diagnostics may be unavailable when even the failure arena cannot be built.
    // Preserve the call's failure category; never infer an exception type here.
    if status != 0 && arena.ptr.is_null() && arena.length == 0 && error.is_null() {
        return failure(status, "managed exception (diagnostic unavailable)".into());
    }
    let mut r = unsafe { Reader::new(arena)? };
    if status == 0 {
        if !error.is_null() {
            return Err(BridgeError::Host("success contains an error".into()));
        }
        return Ok(r);
    }
    let message = r.read(error).and_then(|s| r.string(s))?;
    failure(status, message)
}
fn failure(status: i32, message: String) -> Result<Reader, BridgeError> {
    match status {
        1 => Err(BridgeError::Semantic(message)),
        2 | 4 => Err(BridgeError::Host(format!(
            "managed host/ABI failure ({status}): {message}"
        ))),
        _ => Err(BridgeError::Host(format!(
            "unknown native result status {status}"
        ))),
    }
}
pub trait DecodePayload: Copy {
    type Owned;
    fn decode(self, r: &mut Reader) -> Result<Self::Owned, String>;
}
impl DecodePayload for EvaluateResult {
    type Owned = EvaluationResponse;
    fn decode(self, r: &mut Reader) -> Result<Self::Owned, String> {
        let node = r.read(self.node)?;
        Ok(EvaluationResponse {
            node: decode_WireDecorationNode(node, r, 0)?,
            actions: r.array(self.actions, decode_RuntimeWindowAction, 0)?,
        })
    }
}
impl DecodePayload for InvokeHandlerResult {
    type Owned = HandlerResponse;
    fn decode(self, r: &mut Reader) -> Result<Self::Owned, String> {
        Ok(HandlerResponse {
            node: r.optional(self.node, decode_WireDecorationNode, 0)?,
            actions: r.array(self.actions, decode_RuntimeWindowAction, 0)?,
            invoked: decode_bool(self.invoked)?,
        })
    }
}
impl DecodePayload for EnableResult {
    type Owned = EnableResponse;
    fn decode(self, r: &mut Reader) -> Result<Self::Owned, String> {
        Ok(EnableResponse {
            actions: r.array(self.actions, decode_RuntimeWindowAction, 0)?,
            debug_config: r.optional(self.debug_config, decode_RuntimeDebugConfigUpdate, 0)?,
        })
    }
}
impl DecodePayload for CommitResult {
    type Owned = CommitResponse;
    fn decode(self, r: &mut Reader) -> Result<Self::Owned, String> {
        Ok(CommitResponse {
            debug_config: r.optional(self.debug_config, decode_RuntimeDebugConfigUpdate, 0)?,
        })
    }
}
impl<T: DecodePayload> NativeResult<T> {
    pub fn decode(self, free: ArenaFree) -> Result<T::Owned, BridgeError> {
        let _owner = OwnedManagedArena {
            arena: self.arena,
            free,
        };
        if self.status != 0 && !self.value.is_null() {
            return Err(BridgeError::Host("failure contains a value".into()));
        }
        let mut r = status(self.status, self.arena, self.error)?;
        let value = r.read(self.value)?;
        value.decode(&mut r).map_err(BridgeError::Host)
    }
}
impl SwmStatusResult {
    pub fn decode(self, free: ArenaFree) -> Result<(), BridgeError> {
        // Only a payload-free success may omit its arena. It owns no allocation,
        // so it must not call the managed free export, even with a NULL argument.
        if self.status == 0 && self.arena.ptr.is_null() && self.arena.length == 0 {
            if !self.error.is_null() {
                return Err(BridgeError::Host("success contains an error".into()));
            }
            return Ok(());
        }
        let _owner = OwnedManagedArena {
            arena: self.arena,
            free,
        };
        status(self.status, self.arena, self.error)?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct EvaluationContext {
    pub now_ms: u64,
    pub displays: BTreeMap<String, WaylandOutputSnapshot>,
    pub inputs: BTreeMap<String, RuntimeInputDeviceSnapshot>,
}
impl EvaluationContext {
    fn encode(&self, w: &mut Writer) -> Result<SwmEvaluationContext, String> {
        let displays: Vec<_> = self.displays.iter().collect();
        let displays = w.array(&displays, |(key, value), w| {
            Ok(AbiWaylandOutputEntry {
                key: w.string(key)?,
                value: encode_WaylandOutputSnapshot(value, w)?,
            })
        })?;
        let inputs: Vec<_> = self.inputs.iter().collect();
        let inputs = w.array(&inputs, |(key, value), w| {
            Ok(AbiRuntimeInputEntry {
                key: w.string(key)?,
                value: encode_RuntimeInputDeviceSnapshot(value, w)?,
            })
        })?;
        Ok(SwmEvaluationContext {
            now_ms: self.now_ms,
            displays,
            inputs,
        })
    }
}
// Two-pass input allocation mechanics. No host calls or operation discriminator.
fn borrowed<T>(
    mut encode: impl FnMut(&mut Writer) -> Result<T, String>,
) -> Result<(Writer, T), String> {
    let mut measure = Writer::measure();
    encode(&mut measure)?;
    let mut writer = Writer::allocate(measure.length().max(8))?;
    let input = encode(&mut writer)?;
    Ok((writer, input))
}
#[derive(Debug)]
pub struct EvaluateRequest {
    pub snapshot: WaylandWindowSnapshot,
    pub window_id: Option<String>,
    pub context: EvaluationContext,
}
impl EvaluateRequest {
    pub fn borrowed(&self) -> Result<(Writer, SwmEvaluateInput), String> {
        borrowed(|w| {
            let encoded = encode_WaylandWindowSnapshot(&self.snapshot, w)?;
            Ok(SwmEvaluateInput {
                snapshot: w.put(encoded)?,
                window_id: w.optional(self.window_id.as_ref(), |s, w| w.string(s))?,
                context: self.context.encode(w)?,
            })
        })
    }
}
#[derive(Debug)]
pub struct EvaluatePreviewRequest {
    pub snapshot: WaylandWindowSnapshot,
    pub window_id: Option<String>,
    pub context: EvaluationContext,
}
impl EvaluatePreviewRequest {
    pub fn borrowed(&self) -> Result<(Writer, SwmEvaluatePreviewInput), String> {
        borrowed(|w| {
            let encoded = encode_WaylandWindowSnapshot(&self.snapshot, w)?;
            Ok(SwmEvaluatePreviewInput {
                snapshot: w.put(encoded)?,
                window_id: w.optional(self.window_id.as_ref(), |s, w| w.string(s))?,
                context: self.context.encode(w)?,
            })
        })
    }
}
#[derive(Debug)]
pub struct EvaluateCandidatePreviewRequest {
    pub snapshot: WaylandWindowSnapshot,
    pub window_id: Option<String>,
    pub context: EvaluationContext,
}
impl EvaluateCandidatePreviewRequest {
    pub fn borrowed(&self) -> Result<(Writer, SwmEvaluateCandidatePreviewInput), String> {
        borrowed(|w| {
            let encoded = encode_WaylandWindowSnapshot(&self.snapshot, w)?;
            Ok(SwmEvaluateCandidatePreviewInput {
                snapshot: w.put(encoded)?,
                window_id: w.optional(self.window_id.as_ref(), |s, w| w.string(s))?,
                context: self.context.encode(w)?,
            })
        })
    }
}
#[derive(Debug)]
pub struct EvaluateCachedRequest {
    pub snapshot: Option<WaylandWindowSnapshot>,
    pub window_id: String,
    pub context: EvaluationContext,
}
impl EvaluateCachedRequest {
    pub fn borrowed(&self) -> Result<(Writer, SwmEvaluateCachedInput), String> {
        borrowed(|w| {
            Ok(SwmEvaluateCachedInput {
                snapshot: w.optional(self.snapshot.as_ref(), encode_WaylandWindowSnapshot)?,
                window_id: w.string(&self.window_id)?,
                context: self.context.encode(w)?,
            })
        })
    }
}
#[derive(Debug)]
pub struct InvokeHandlerRequest {
    pub window_id: String,
    pub handler_id: String,
    pub context: EvaluationContext,
}
impl InvokeHandlerRequest {
    pub fn borrowed(&self) -> Result<(Writer, SwmInvokeHandlerInput), String> {
        borrowed(|w| {
            Ok(SwmInvokeHandlerInput {
                window_id: w.string(&self.window_id)?,
                handler_id: w.string(&self.handler_id)?,
                context: self.context.encode(w)?,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        mem::{align_of, offset_of, size_of},
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn empty_ack_needs_no_free_and_does_not_relax_payload_validation() {
        static FREES: AtomicUsize = AtomicUsize::new(0);
        extern "system" fn free(_: ResultArena) {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        let empty = ResultArena {
            ptr: std::ptr::null(),
            length: 0,
        };
        for _ in 0..1000 {
            SwmStatusResult {
                status: 0,
                arena: empty,
                error: std::ptr::null(),
            }
            .decode(free)
            .unwrap();
        }
        assert_eq!(FREES.load(Ordering::SeqCst), 0);
        assert!(
            SwmStatusResult {
                status: 0,
                arena: empty,
                error: std::ptr::dangling(),
            }
            .decode(free)
            .is_err()
        );
        assert_eq!(FREES.load(Ordering::SeqCst), 0);
        assert!(
            SwmStatusResult {
                status: 0,
                arena: ResultArena { length: 1, ..empty },
                error: std::ptr::null(),
            }
            .decode(free)
            .is_err()
        );
        // Payload-bearing successes still require a valid arena and root.
        assert!(
            SwmEvaluateResult {
                status: 0,
                arena: empty,
                value: std::ptr::null(),
                error: std::ptr::null(),
            }
            .decode(free)
            .is_err()
        );
        // The existing error cleanup path, including NULL failure arenas, remains.
        assert!(matches!(
            SwmStatusResult {
                status: 1,
                arena: empty,
                error: std::ptr::null(),
            }
            .decode(free),
            Err(BridgeError::Semantic(_))
        ));
        assert_eq!(FREES.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn operation_layouts() {
        assert_eq!(size_of::<usize>(), 8);
        assert_eq!(size_of::<ResultArena>(), 16);
        assert_eq!(align_of::<ResultArena>(), 8);
        assert_eq!(offset_of!(ResultArena, length), 8);
        assert_eq!(size_of::<ArenaString>(), 16);
        assert_eq!(offset_of!(ArenaString, length), 8);
        assert_eq!(size_of::<SwmEvaluationContext>(), 40);
        assert_eq!(offset_of!(SwmEvaluationContext, displays), 8);
        assert_eq!(offset_of!(SwmEvaluationContext, inputs), 24);
        assert_eq!(size_of::<SwmEvaluateInput>(), 56);
        assert_eq!(align_of::<SwmEvaluateInput>(), 8);
        assert_eq!(offset_of!(SwmEvaluateInput, context), 16);
        assert_eq!(size_of::<SwmEvaluatePreviewInput>(), 56);
        assert_eq!(size_of::<SwmEvaluateCandidatePreviewInput>(), 56);
        assert_eq!(size_of::<SwmEvaluateCachedInput>(), 64);
        assert_eq!(offset_of!(SwmEvaluateCachedInput, context), 24);
        assert_eq!(size_of::<SwmInvokeHandlerInput>(), 72);
        assert_eq!(offset_of!(SwmInvokeHandlerInput, handler_id), 16);
        assert_eq!(offset_of!(SwmInvokeHandlerInput, context), 32);
        assert_eq!(size_of::<SwmEvaluateResult>(), 40);
        assert_eq!(align_of::<SwmEvaluateResult>(), 8);
        assert_eq!(offset_of!(SwmEvaluateResult, arena), 8);
        assert_eq!(offset_of!(SwmEvaluateResult, value), 24);
        assert_eq!(offset_of!(SwmEvaluateResult, error), 32);
        assert_eq!(size_of::<SwmInvokeHandlerResult>(), 40);
        assert_eq!(size_of::<SwmLifecycleEnableResult>(), 40);
        assert_eq!(size_of::<SwmCommitAssemblyResult>(), 40);
        assert_eq!(size_of::<SwmStatusResult>(), 32);
        assert_eq!(offset_of!(SwmStatusResult, error), 24);
        assert_eq!(size_of::<EvaluateResult>(), 24);
        assert_eq!(offset_of!(EvaluateResult, actions), 8);
        assert_eq!(size_of::<InvokeHandlerResult>(), 32);
        assert_eq!(offset_of!(InvokeHandlerResult, invoked), 24);
        assert_eq!(size_of::<EnableResult>(), 24);
        assert_eq!(offset_of!(EnableResult, debug_config), 16);
        assert_eq!(size_of::<CommitResult>(), 8);
        assert_eq!(size_of::<Slice<AbiWireDecorationNode>>(), 16);
        assert_eq!(align_of::<Slice<AbiWireDecorationNode>>(), 8);
        assert_eq!(offset_of!(Slice<AbiWireDecorationNode>, length), 8);
        assert_eq!(size_of::<Slice<AbiRuntimeWindowAction>>(), 16);
        assert_eq!(size_of::<AbiDimension>(), 24);
        assert_eq!(size_of::<AbiClickDescriptor>(), 24);
        assert_eq!(size_of::<AbiFontFamily>(), 16);
        assert_eq!(size_of::<AbiResizeHitArea>(), 16);
    }
    #[test]
    fn decoder_rejects_cyclic_children_and_invalid_action_tags() {
        let mut words = [0u64; 256];
        let base = words.as_mut_ptr().cast::<u8>();
        let arena = ResultArena {
            ptr: base,
            length: 2048,
        };
        let node = base.cast::<AbiWireDecorationNode>();
        unsafe {
            node.write(std::mem::zeroed());
            (*node).children = Slice {
                ptr: node,
                length: 1,
            };
        }
        let mut reader = unsafe { Reader::new(arena).unwrap() };
        let raw = reader.read(node).unwrap();
        assert!(
            decode_WireDecorationNode(raw, &mut reader, 0)
                .unwrap_err()
                .contains("depth")
        );
        let raw = AbiRuntimeWindowAction {
            window_id: ArenaString {
                ptr: std::ptr::null(),
                length: 0,
            },
            action: u32::MAX,
            channel: std::ptr::null(),
        };
        let mut reader = unsafe { Reader::new(arena).unwrap() };
        assert!(
            decode_RuntimeWindowAction(raw, &mut reader, 0)
                .unwrap_err()
                .contains("tag")
        );
    }
    #[test]
    fn typed_payloads_copy_before_free_and_free_once_on_every_failure() {
        static FREES: AtomicUsize = AtomicUsize::new(0);
        extern "system" fn free(arena: ResultArena) {
            FREES.fetch_add(1, Ordering::SeqCst);
            if !arena.ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(arena.ptr.cast_mut().cast::<[u64; 256]>()));
                }
            }
        }
        fn allocation() -> (*mut u8, ResultArena) {
            let ptr = Box::into_raw(Box::new([0u64; 256])).cast::<u8>();
            (ptr, ResultArena { ptr, length: 2048 })
        }
        let (ptr, arena) = allocation();
        let root = ptr.cast::<EvaluateResult>();
        let node = unsafe { ptr.add(64).cast::<AbiWireDecorationNode>() };
        unsafe {
            node.write(std::mem::zeroed());
            std::ptr::copy_nonoverlapping(b"Window".as_ptr(), ptr.add(640), 6);
            (*node).kind = ArenaString {
                ptr: ptr.add(640),
                length: 6,
            };
            root.write(EvaluateResult {
                node,
                actions: Slice {
                    ptr: std::ptr::null(),
                    length: 0,
                },
            });
        }
        let owned = SwmEvaluateResult {
            status: 0,
            arena,
            value: root,
            error: std::ptr::null(),
        }
        .decode(free)
        .unwrap();
        assert_eq!(owned.node.kind, "Window"); // Actual allocation has already been freed.
        let (ptr, arena) = allocation();
        let root = ptr.cast::<InvokeHandlerResult>();
        unsafe {
            root.write(InvokeHandlerResult {
                node: std::ptr::null(),
                actions: Slice {
                    ptr: std::ptr::null(),
                    length: 0,
                },
                invoked: 1,
            });
        }
        let owned = SwmInvokeHandlerResult {
            status: 0,
            arena,
            value: root,
            error: std::ptr::null(),
        }
        .decode(free)
        .unwrap();
        assert!(owned.invoked && owned.node.is_none());
        let (ptr, arena) = allocation();
        let root = ptr.cast::<EnableResult>();
        unsafe {
            root.write(EnableResult {
                actions: Slice {
                    ptr: std::ptr::null(),
                    length: 0,
                },
                debug_config: std::ptr::null(),
            });
        }
        assert!(
            SwmLifecycleEnableResult {
                status: 0,
                arena,
                value: root,
                error: std::ptr::null()
            }
            .decode(free)
            .unwrap()
            .debug_config
            .is_none()
        );
        let (ptr, arena) = allocation();
        let root = ptr.cast::<CommitResult>();
        unsafe {
            root.write(CommitResult {
                debug_config: std::ptr::null(),
            });
        }
        assert!(
            SwmCommitAssemblyResult {
                status: 0,
                arena,
                value: root,
                error: std::ptr::null()
            }
            .decode(free)
            .unwrap()
            .debug_config
            .is_none()
        );
        let (_, arena) = allocation();
        SwmStatusResult {
            status: 0,
            arena,
            error: std::ptr::null(),
        }
        .decode(free)
        .unwrap();
        let (ptr, arena) = allocation();
        let error = unsafe { ptr.add(64).cast::<ArenaString>() };
        unsafe {
            error.write(ArenaString {
                ptr: ptr.add(96),
                length: 1,
            });
            ptr.add(96).write(b'x');
        }
        assert!(matches!(
            SwmEvaluateResult {
                status: 1,
                arena,
                value: std::ptr::null(),
                error
            }
            .decode(free),
            Err(BridgeError::Semantic(_))
        ));
        let (ptr, arena) = allocation();
        assert!(
            SwmEvaluateResult {
                status: 0,
                arena,
                value: ptr.wrapping_add(2047).cast(),
                error: std::ptr::null()
            }
            .decode(free)
            .is_err()
        );
        let (_, arena) = allocation();
        {
            let _guard = OwnedManagedArena { arena, free };
        }
        assert!(matches!(
            SwmStatusResult {
                status: 1,
                arena: ResultArena {
                    ptr: std::ptr::null(),
                    length: 0
                },
                error: std::ptr::null()
            }
            .decode(free),
            Err(BridgeError::Semantic(message)) if message.contains("diagnostic unavailable")
        ));
        let (_, arena) = allocation();
        assert!(
            SwmEvaluateResult {
                status: 1,
                arena,
                value: std::ptr::null(),
                error: arena.ptr.wrapping_sub(8).cast()
            }
            .decode(free)
            .is_err()
        );
        assert_eq!(FREES.load(Ordering::SeqCst), 10);
    }
}
