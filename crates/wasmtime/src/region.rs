use crate::{Endian, GuestError, Le};
use std::marker;
use std::mem;

pub struct BorrowChecker<'a> {
    _marker: marker::PhantomData<&'a mut [u8]>,
    ptr: *mut u8,
    len: usize,
}

// These are not automatically implemented with our storage of `*mut u8`, so we
// need to manually declare that this type is threadsafe.
unsafe impl Send for BorrowChecker<'_> {}
unsafe impl Sync for BorrowChecker<'_> {}

fn to_trap(err: impl std::error::Error + Send + Sync + 'static) -> anyhow::Error {
    anyhow::anyhow!(Box::new(err) as Box<dyn std::error::Error + Send + Sync>)
}

impl<'a> BorrowChecker<'a> {
    pub fn new(data: &'a mut [u8]) -> BorrowChecker<'a> {
        BorrowChecker {
            ptr: data.as_mut_ptr(),
            len: data.len(),
            _marker: marker::PhantomData,
        }
    }

    pub fn slice<T: AllBytesValid>(&mut self, ptr: i32, len: i32) -> anyhow::Result<&'a [T]> {
        let (ret, _) = self.get_slice(ptr, len)?;
        // SAFETY: We're promoting the valid lifetime of `ret` from a temporary
        // borrow on `self` to `'a` on this `BorrowChecker`. At the same time
        // we're recording that this is a persistent shared borrow (until this
        // borrow checker is deleted), which disallows future mutable borrows
        // of the same data.
        let ret = unsafe { &*(ret as *const [T]) };
        Ok(ret)
    }

    fn get_slice<T: AllBytesValid>(&self, ptr: i32, len: i32) -> anyhow::Result<(&[T], Region)> {
        let r = self.region::<T>(ptr, len)?;
        Ok((
            // SAFETY: invariants to uphold:
            //
            // * The lifetime of the input is valid for the lifetime of the
            //   output. In this case we're threading through the lifetime
            //   of `&self` to the output.
            // * The actual output is valid, which is guaranteed with the
            //   `AllBytesValid` bound.
            // * We uphold Rust's borrowing guarantees, namely that this
            //   borrow we're returning isn't overlapping with any mutable
            //   borrows.
            // * The region `r` we're returning accurately describes the
            //   slice we're returning in wasm linear memory.
            unsafe {
                std::slice::from_raw_parts(self.ptr.add(r.start as usize) as *const T, len as usize)
            },
            r,
        ))
    }

    fn region<T>(&self, ptr: i32, len: i32) -> anyhow::Result<Region> {
        assert_eq!(std::mem::align_of::<T>(), 1);
        let r = Region {
            start: ptr as u32,
            len: (len as u32)
                .checked_mul(mem::size_of::<T>() as u32)
                .ok_or_else(|| to_trap(GuestError::PtrOverflow))?,
        };
        self.validate_contains(&r)?;
        Ok(r)
    }

    pub fn slice_str(&mut self, ptr: i32, len: i32) -> anyhow::Result<&'a str> {
        let bytes = self.slice(ptr, len)?;
        std::str::from_utf8(bytes).map_err(to_trap)
    }

    fn validate_contains(&self, region: &Region) -> anyhow::Result<()> {
        let end = region
            .start
            .checked_add(region.len)
            .ok_or_else(|| to_trap(GuestError::PtrOverflow))? as usize;
        if end <= self.len {
            Ok(())
        } else {
            Err(to_trap(GuestError::PtrOutOfBounds(*region)))
        }
    }

    pub fn raw(&self) -> *mut [u8] {
        std::ptr::slice_from_raw_parts_mut(self.ptr, self.len)
    }

    pub fn load<T: Endian>(&self, offset: i32) -> anyhow::Result<T> {
        let (slice, _) = self.get_slice::<Le<T>>(offset, 1)?;
        Ok(slice[0].get())
    }
}

/// Unsafe trait representing types where every byte pattern is valid for their
/// representation.
///
/// This is the set of types which wasmtime can have a raw pointer to for
/// values which reside in wasm linear memory.
pub unsafe trait AllBytesValid {}

unsafe impl AllBytesValid for u8 {}
unsafe impl AllBytesValid for u16 {}
unsafe impl AllBytesValid for u32 {}
unsafe impl AllBytesValid for u64 {}
unsafe impl AllBytesValid for i8 {}
unsafe impl AllBytesValid for i16 {}
unsafe impl AllBytesValid for i32 {}
unsafe impl AllBytesValid for i64 {}
unsafe impl AllBytesValid for f32 {}
unsafe impl AllBytesValid for f64 {}

macro_rules! tuples {
    ($(($($t:ident)*))*) => ($(
        unsafe impl <$($t:AllBytesValid,)*> AllBytesValid for ($($t,)*) {}
    )*)
}

tuples! {
    ()
    (T1)
    (T1 T2)
    (T1 T2 T3)
    (T1 T2 T3 T4)
    (T1 T2 T3 T4 T5)
    (T1 T2 T3 T4 T5 T6)
    (T1 T2 T3 T4 T5 T6 T7)
    (T1 T2 T3 T4 T5 T6 T7 T8)
    (T1 T2 T3 T4 T5 T6 T7 T8 T9)
    (T1 T2 T3 T4 T5 T6 T7 T8 T9 T10)
}

/// Represents a contiguous region in memory.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Region {
    pub start: u32,
    pub len: u32,
}
