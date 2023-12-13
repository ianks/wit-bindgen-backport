pub use wit_bindgen_wasmtime_impl::{export, import};

#[cfg(feature = "async")]
pub use async_trait::async_trait;
#[cfg(feature = "tracing-lib")]
pub use tracing_lib as tracing;
#[doc(hidden)]
pub use {anyhow, bitflags, wasmtime};

mod error;
mod le;
mod region;
mod slab;
mod table;

pub use error::GuestError;
pub use le::{Endian, Le};
pub use region::{AllBytesValid, BorrowChecker, Region};
pub use table::*;

#[doc(hidden)]
pub mod rt {
    use crate::slab::Slab;
    use crate::{Endian, Le};
    use std::mem;
    use wasmtime::*;

    pub trait RawMem {
        fn store<T: Endian>(&mut self, offset: i32, val: T) -> anyhow::Result<()>;
        fn store_many<T: Endian>(&mut self, offset: i32, vals: &[T]) -> anyhow::Result<()>;
        fn load<T: Endian>(&self, offset: i32) -> anyhow::Result<T>;
    }

    impl RawMem for [u8] {
        fn store<T: Endian>(&mut self, offset: i32, val: T) -> anyhow::Result<()> {
            let mem = self
                .get_mut(offset as usize..)
                .and_then(|m| m.get_mut(..mem::size_of::<T>()))
                .ok_or_else(|| anyhow::anyhow!("out of bounds write"))?;
            Le::from_slice_mut(mem)[0].set(val);
            Ok(())
        }

        fn store_many<T: Endian>(&mut self, offset: i32, val: &[T]) -> anyhow::Result<()> {
            let mem = self
                .get_mut(offset as usize..)
                .and_then(|m| {
                    let len = mem::size_of::<T>().checked_mul(val.len())?;
                    m.get_mut(..len)
                })
                .ok_or_else(|| anyhow::anyhow!("out of bounds write"))?;
            for (slot, val) in Le::from_slice_mut(mem).iter_mut().zip(val) {
                slot.set(*val);
            }
            Ok(())
        }

        fn load<T: Endian>(&self, offset: i32) -> anyhow::Result<T> {
            let mem = self
                .get(offset as usize..)
                .and_then(|m| m.get(..mem::size_of::<Le<T>>()))
                .ok_or_else(|| anyhow::anyhow!("out of bounds read"))?;
            Ok(Le::from_slice(mem)[0].get())
        }
    }

    pub fn char_from_i32(val: i32) -> anyhow::Result<char> {
        core::char::from_u32(val as u32)
            .ok_or_else(|| anyhow::anyhow!("char value out of valid range"))
    }

    pub fn invalid_variant(name: &str) -> anyhow::Error {
        anyhow::anyhow!("invalid discriminant for `{}`", name)
    }

    pub fn validate_flags<T, U>(
        bits: T,
        all: T,
        name: &str,
        mk: impl FnOnce(T) -> U,
    ) -> anyhow::Result<U>
    where
        T: std::ops::Not<Output = T> + std::ops::BitAnd<Output = T> + From<u8> + PartialEq + Copy,
    {
        if bits & !all != 0u8.into() {
            let msg = format!("invalid flags specified for `{}`", name);
            anyhow::bail!(msg)
        } else {
            Ok(mk(bits))
        }
    }

    pub fn get_func<T, Args: WasmParams, Results: WasmResults>(caller: &mut Caller<'_, T>, func: &str) -> anyhow::Result<TypedFunc<Args, Results>> {
        let func = caller
            .get_export(func)
            .ok_or_else(|| {
                let msg = format!("`{}` export not available", func);
                anyhow::anyhow!(msg)
            })?
            .into_func()
            .map(|f| unsafe {
                TypedFunc::new_unchecked(f)
            })
            .ok_or_else(|| {
                let msg = format!("`{}` export not a function", func);
                anyhow::anyhow!(msg)
            })?;
        Ok(func)
    }

    pub fn get_memory<T>(caller: &mut Caller<'_, T>, mem: &str) -> anyhow::Result<Memory> {
        let mem = caller
            .get_export(mem)
            .ok_or_else(|| {
                let msg = format!("`{}` export not available", mem);
                anyhow::anyhow!(msg)
            })?
            .into_memory()
            .ok_or_else(|| {
                let msg = format!("`{}` export not a memory", mem);
                anyhow::anyhow!(msg)
            })?;
        Ok(mem)
    }

    pub fn bad_int(_: std::num::TryFromIntError) -> anyhow::Error {
        let msg = "out-of-bounds integer conversion";
        anyhow::anyhow!(msg)
    }

    pub fn copy_slice<T: Endian>(
        store: impl AsContextMut,
        memory: &Memory,
        base: i32,
        len: i32,
        _align: i32,
    ) -> anyhow::Result<Vec<T>> {
        let size = (len as u32)
            .checked_mul(mem::size_of::<T>() as u32)
            .ok_or_else(|| anyhow::anyhow!("array too large to fit in wasm memory"))?;
        let slice = memory
            .data(&store)
            .get(base as usize..)
            .and_then(|s| s.get(..size as usize))
            .ok_or_else(|| anyhow::anyhow!("out of bounds read"))?;
        Ok(Le::from_slice(slice).iter().map(|s| s.get()).collect())
    }

    macro_rules! as_traits {
        ($(($name:ident $tr:ident $ty:ident ($($tys:ident)*)))*) => ($(
            pub fn $name<T: $tr>(t: T) -> $ty {
                t.$name()
            }

            pub trait $tr {
                fn $name(self) -> $ty;
            }

            impl<'a, T: Copy + $tr> $tr for &'a T {
                fn $name(self) -> $ty {
                    (*self).$name()
                }
            }

            $(
                impl $tr for $tys {
                    #[inline]
                    fn $name(self) -> $ty {
                        self as $ty
                    }
                }
            )*
        )*)
    }

    as_traits! {
        (as_i32 AsI32 i32 (char i8 u8 i16 u16 i32 u32))
        (as_i64 AsI64 i64 (i64 u64))
        (as_f32 AsF32 f32 (f32))
        (as_f64 AsF64 f64 (f64))
    }

    #[derive(Default, Debug)]
    pub struct IndexSlab {
        slab: Slab<ResourceIndex>,
    }

    impl IndexSlab {
        pub fn insert(&mut self, resource: ResourceIndex) -> u32 {
            self.slab.insert(resource)
        }

        pub fn get(&self, slab_idx: u32) -> anyhow::Result<ResourceIndex> {
            match self.slab.get(slab_idx) {
                Some(idx) => Ok(*idx),
                None => anyhow::bail!("invalid index specified for handle"),
            }
        }

        pub fn remove(&mut self, slab_idx: u32) -> anyhow::Result<ResourceIndex> {
            match self.slab.remove(slab_idx) {
                Some(idx) => Ok(idx),
                None => anyhow::bail!("invalid index specified for handle"),
            }
        }
    }

    #[derive(Default, Debug)]
    pub struct ResourceSlab {
        slab: Slab<Resource>,
    }

    #[derive(Debug)]
    struct Resource {
        wasm: i32,
        refcnt: u32,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct ResourceIndex(u32);

    impl ResourceSlab {
        pub fn insert(&mut self, wasm: i32) -> ResourceIndex {
            ResourceIndex(self.slab.insert(Resource { wasm, refcnt: 1 }))
        }

        pub fn get(&self, idx: ResourceIndex) -> i32 {
            self.slab.get(idx.0).unwrap().wasm
        }

        pub fn clone(&mut self, idx: ResourceIndex) -> anyhow::Result<()> {
            let resource = self.slab.get_mut(idx.0).unwrap();
            resource.refcnt = match resource.refcnt.checked_add(1) {
                Some(cnt) => cnt,
                None => anyhow::bail!("resource index count overflow"),
            };
            Ok(())
        }

        pub fn drop(&mut self, idx: ResourceIndex) -> Option<i32> {
            let resource = self.slab.get_mut(idx.0).unwrap();
            assert!(resource.refcnt > 0);
            resource.refcnt -= 1;
            if resource.refcnt != 0 {
                return None;
            }
            let resource = self.slab.remove(idx.0).unwrap();
            Some(resource.wasm)
        }
    }
}
