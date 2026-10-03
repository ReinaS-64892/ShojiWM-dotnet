//! Local ABI v6 memory primitives. Foreign pointers never leave the host thread.
#![allow(non_snake_case)]
use std::mem::{align_of, size_of};
pub const MAX_ARENA_SIZE: usize = 8 * 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ResultArena {
    pub ptr: *const u8,
    pub length: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ArenaString {
    pub ptr: *const u8,
    pub length: i32,
}
#[repr(C)]
pub struct Slice<T> {
    pub ptr: *const T,
    pub length: i32,
}
impl<T> Copy for Slice<T> {}
impl<T> Clone for Slice<T> {
    fn clone(&self) -> Self {
        *self
    }
}
pub fn decode_f32(v: f32) -> Result<f32, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err("non-finite ABI number".into())
    }
}
pub fn decode_f64(v: f64) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err("non-finite ABI number".into())
    }
}
pub fn decode_bool(v: u8) -> Result<bool, String> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid boolean".into()),
    }
}

pub struct Reader {
    base: usize,
    end: usize,
    remaining: usize,
}
impl Reader {
    /// Caller guarantees that the base describes a live allocation. Range checks
    /// cannot establish allocation provenance of a foreign base address.
    pub unsafe fn new(arena: ResultArena) -> Result<Self, String> {
        if arena.length <= 0 || arena.length as usize > MAX_ARENA_SIZE || arena.ptr.is_null() {
            return Err("invalid arena or exceeds 8 MiB limit".into());
        }
        let base = arena.ptr as usize;
        let end = base
            .checked_add(arena.length as usize)
            .ok_or("arena address overflow")?;
        Ok(Self {
            base,
            end,
            remaining: MAX_ARENA_SIZE,
        })
    }
    pub fn depth(&self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            Err("native graph maximum depth exceeded".into())
        } else {
            Ok(())
        }
    }
    fn range<T>(&mut self, ptr: *const T, length: i32) -> Result<usize, String> {
        let length = usize::try_from(length).map_err(|_| "negative slice length")?;
        let bytes = size_of::<T>()
            .checked_mul(length)
            .ok_or("slice size overflow")?;
        if bytes == 0 && ptr.is_null() {
            return Ok(0);
        }
        let address = ptr as usize;
        let end = address.checked_add(bytes).ok_or("pointer overflow")?;
        if address < self.base || end > self.end || address % align_of::<T>() != 0 {
            return Err("pointer outside arena or misaligned".into());
        }
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or("native graph decode budget exceeded")?;
        Ok(length)
    }
    pub fn read<T: Copy>(&mut self, ptr: *const T) -> Result<T, String> {
        self.range(ptr, 1)?;
        // SAFETY: checked aligned readable range within the owner's live allocation.
        Ok(unsafe { ptr.read() })
    }
    pub fn string(&mut self, v: ArenaString) -> Result<String, String> {
        let length = self.range(v.ptr, v.length)?;
        if length == 0 {
            return Ok(String::new());
        }
        let bytes = unsafe { std::slice::from_raw_parts(v.ptr, length) };
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|e| format!("invalid arena UTF-8: {e}"))
    }
    pub fn optional<T: Copy, U>(
        &mut self,
        ptr: *const T,
        decode: impl FnOnce(T, &mut Self, usize) -> Result<U, String>,
        depth: usize,
    ) -> Result<Option<U>, String> {
        self.depth(depth)?;
        if ptr.is_null() {
            Ok(None)
        } else {
            let v = self.read(ptr)?;
            decode(v, self, depth).map(Some)
        }
    }
    pub fn array<T: Copy, U>(
        &mut self,
        v: Slice<T>,
        mut decode: impl FnMut(T, &mut Self, usize) -> Result<U, String>,
        depth: usize,
    ) -> Result<Vec<U>, String> {
        self.depth(depth)?;
        let count = self.range(v.ptr, v.length)?;
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            result.push(decode(unsafe { v.ptr.add(i).read() }, self, depth)?);
        }
        Ok(result)
    }
}

// Rust-owned borrowed-input arena. Allocated only on the runtime thread.
// Measure pass never dereferences temporary pointers; all fields are initialized
// in the second pass and its allocation is retained until the unmanaged return.
pub struct Writer {
    memory: Option<Box<[u64]>>,
    offset: usize,
    capacity: usize,
}
impl Writer {
    pub fn measure() -> Self {
        Self {
            memory: None,
            offset: 0,
            capacity: MAX_ARENA_SIZE,
        }
    }
    pub fn allocate(size: usize) -> Result<Self, String> {
        if size == 0 || size > MAX_ARENA_SIZE {
            return Err("input arena exceeds 8 MiB limit".into());
        }
        Ok(Self {
            memory: Some(vec![0u64; size.div_ceil(8)].into_boxed_slice()),
            offset: 0,
            capacity: size,
        })
    }
    pub fn length(&self) -> usize {
        self.offset
    }
    pub fn put<T: Copy>(&mut self, value: T) -> Result<*const T, String> {
        let ptr = self.slots::<T>(1)?;
        if !ptr.is_null() {
            unsafe { (ptr as *mut T).write(value) };
        }
        Ok(ptr)
    }
    fn slots<T>(&mut self, count: usize) -> Result<*const T, String> {
        if align_of::<T>() > 8 {
            return Err("unsupported input alignment".into());
        }
        let start = self.offset.checked_add(7).ok_or("input size overflow")? & !7;
        self.offset = start
            .checked_add(
                size_of::<T>()
                    .checked_mul(count)
                    .ok_or("input size overflow")?,
            )
            .ok_or("input size overflow")?;
        if self.offset > self.capacity {
            return Err("input arena exceeds 8 MiB limit".into());
        }
        Ok(match &self.memory {
            Some(memory) if count != 0 => unsafe { memory.as_ptr().cast::<u8>().add(start).cast() },
            _ => std::ptr::null(),
        })
    }
    pub fn string(&mut self, text: &str) -> Result<ArenaString, String> {
        let ptr = self.slots::<u8>(text.len())?;
        if !ptr.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), ptr as *mut u8, text.len()) };
        }
        Ok(ArenaString {
            ptr,
            length: text.len() as i32,
        })
    }
    pub fn optional<T, U: Copy>(
        &mut self,
        value: Option<&T>,
        encode: impl FnOnce(&T, &mut Self) -> Result<U, String>,
    ) -> Result<*const U, String> {
        match value {
            Some(v) => {
                let encoded = encode(v, self)?;
                self.put(encoded)
            }
            None => Ok(std::ptr::null()),
        }
    }
    pub fn array<T, U: Copy>(
        &mut self,
        values: &[T],
        mut encode: impl FnMut(&T, &mut Self) -> Result<U, String>,
    ) -> Result<Slice<U>, String> {
        let ptr = self.slots::<U>(values.len())?;
        for (i, value) in values.iter().enumerate() {
            let encoded = encode(value, self)?;
            if !ptr.is_null() {
                unsafe { (ptr as *mut U).add(i).write(encoded) };
            }
        }
        Ok(Slice {
            ptr,
            length: values.len() as i32,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiDimension {
    pub kind: u32,
    pub pixels: i32,
    pub keyword: ArenaString,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiFontFamily {
    pub names: Slice<ArenaString>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiClickDescriptor {
    pub kind: u32,
    pub action: u32,
    pub handler: ArenaString,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AbiResizeHitArea {
    pub edge_px: i32,
    pub corner_px: *const i32,
}
pub fn decode_Dimension(
    v: AbiDimension,
    r: &mut Reader,
    _: usize,
) -> Result<shojiwm_lib::ssd::bridge::WireDimension, String> {
    use shojiwm_lib::ssd::bridge::WireDimension;
    match v.kind {
        0 => Ok(WireDimension::Pixels(v.pixels as f64)),
        1 => Ok(WireDimension::Keyword(r.string(v.keyword)?)),
        _ => Err("invalid dimension tag".into()),
    }
}
pub fn decode_FontFamily(
    v: AbiFontFamily,
    r: &mut Reader,
    depth: usize,
) -> Result<shojiwm_lib::ssd::bridge::WireFontFamily, String> {
    Ok(shojiwm_lib::ssd::bridge::WireFontFamily::Multiple(
        r.array(v.names, |v, r, _| r.string(v), depth)?,
    ))
}
pub fn decode_ClickDescriptor(
    v: AbiClickDescriptor,
    r: &mut Reader,
    _: usize,
) -> Result<shojiwm_lib::ssd::bridge::WireOnClick, String> {
    use shojiwm_lib::ssd::bridge::*;
    match v.kind {
        0 => Ok(WireOnClick::Action(match v.action {
            0 => WireWindowAction::Close,
            1 => WireWindowAction::Maximize,
            2 => WireWindowAction::Unmaximize,
            3 => WireWindowAction::Minimize,
            _ => return Err("invalid action tag".into()),
        })),
        1 => Ok(WireOnClick::RuntimeHandler(WireRuntimeHandler {
            kind: "runtime-handler".into(),
            id: r.string(v.handler)?,
        })),
        _ => Err("invalid click tag".into()),
    }
}
pub fn decode_ResizeHitArea(
    v: AbiResizeHitArea,
    r: &mut Reader,
    depth: usize,
) -> Result<shojiwm_lib::ssd::bridge::WireResizeHitArea, String> {
    use shojiwm_lib::ssd::bridge::*;
    Ok(WireResizeHitArea::Detailed(WireResizeHitAreaFields {
        edge_px: Some(v.edge_px),
        corner_px: r.optional(v.corner_px, |v, _, _| Ok(v), depth)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reader(memory: &[u64]) -> Reader {
        unsafe {
            Reader::new(ResultArena {
                ptr: memory.as_ptr().cast(),
                length: std::mem::size_of_val(memory) as i32,
            })
            .unwrap()
        }
    }
    #[test]
    fn invalid_ranges_lengths_alignment_utf8_and_tags() {
        let memory = [0u64; 8];
        let base = memory.as_ptr().cast::<u8>();
        for (ptr, length) in [
            (base.wrapping_sub(1), 1),
            (base.wrapping_add(65), 0),
            (base.wrapping_add(63), 2),
            (base, -1),
            (std::ptr::null(), 1),
            (usize::MAX as *const u8, 2),
        ] {
            assert!(reader(&memory).string(ArenaString { ptr, length }).is_err());
        }
        assert!(
            reader(&memory)
                .read(base.wrapping_add(1).cast::<u64>())
                .is_err()
        );
        assert!(
            reader(&memory)
                .array(
                    Slice {
                        ptr: base.cast::<u64>(),
                        length: i32::MAX
                    },
                    |v, _, _| Ok(v),
                    0
                )
                .is_err()
        );
        let invalid = [0xffff_ffff_ffff_ffffu64];
        assert!(
            reader(&invalid)
                .string(ArenaString {
                    ptr: invalid.as_ptr().cast(),
                    length: 1
                })
                .is_err()
        );
        assert!(decode_bool(2).is_err());
        assert!(decode_f32(f32::NAN).is_err());
        assert!(decode_f64(f64::INFINITY).is_err());
        assert!(
            decode_Dimension(
                AbiDimension {
                    kind: 99,
                    pixels: 0,
                    keyword: ArenaString {
                        ptr: std::ptr::null(),
                        length: 0
                    }
                },
                &mut reader(&memory),
                0
            )
            .is_err()
        );
        assert!(reader(&memory).depth(MAX_DEPTH + 1).is_err());
        assert!(
            unsafe {
                Reader::new(ResultArena {
                    ptr: usize::MAX as *const u8,
                    length: 8,
                })
            }
            .is_err()
        );
        assert!(
            unsafe {
                Reader::new(ResultArena {
                    ptr: base,
                    length: MAX_ARENA_SIZE as i32 + 1,
                })
            }
            .is_err()
        );
    }
    #[test]
    fn borrowed_arena_alignment_utf8_empty_and_optional() {
        let mut measure = Writer::measure();
        measure.string("日本語🙂").unwrap();
        measure.put(1.5f64).unwrap();
        let mut writer = Writer::allocate(measure.length()).unwrap();
        let text = writer.string("日本語🙂").unwrap();
        let number = writer.put(1.5f64).unwrap();
        let mut r = unsafe {
            Reader::new(ResultArena {
                ptr: writer.memory.as_ref().unwrap().as_ptr().cast(),
                length: writer.length() as i32,
            })
            .unwrap()
        };
        assert_eq!(r.string(text).unwrap(), "日本語🙂");
        assert_eq!(r.read(number).unwrap(), 1.5);
        assert_eq!(
            r.string(ArenaString {
                ptr: std::ptr::null(),
                length: 0
            })
            .unwrap(),
            ""
        );
        assert_eq!(
            r.optional::<u32, u32>(std::ptr::null(), |v, _, _| Ok(v), 0)
                .unwrap(),
            None
        );
    }
}
